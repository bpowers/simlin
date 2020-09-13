// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast;
use crate::common::{Ident, Result};
use crate::interpreter::{BinaryOp, UnaryOp};
use crate::model::Model;
use crate::variable::Variable;
use crate::xmile;
use crate::Project;
use std::borrow::BorrowMut;

const TIME_OFF: usize = 0;

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

const DEFAULT_DT: xmile::Dt = xmile::Dt {
    value: 1.0,
    reciprocal: None,
};

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
    pub fn from(specs: &xmile::SimSpecs) -> Self {
        let dt: f64 = {
            let spec_dt = specs.dt.as_ref().unwrap_or(&DEFAULT_DT);
            if spec_dt.reciprocal.unwrap_or(false) {
                1.0 / spec_dt.value
            } else {
                spec_dt.value
            }
        };

        let save_step: f64 = specs.save_step.unwrap_or(dt);

        let method = if specs.method.is_none() {
            Method::Euler
        } else {
            let method_str = specs.method.as_ref().unwrap();
            match method_str.to_lowercase().as_str() {
                "euler" => Method::Euler,
                _ => {
                    eprintln!(
                        "warning, simulation requested '{}' method, but only support Euler",
                        method_str
                    );
                    Method::Euler
                }
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

#[derive(PartialEq, Clone, Debug)]
pub enum BuiltinFn {
    Lookup(String, Box<Expr>),
    Abs(Box<Expr>),
    Arccos(Box<Expr>),
    Arcsin(Box<Expr>),
    Arctan(Box<Expr>),
    Cos(Box<Expr>),
    Exp(Box<Expr>),
    Inf,
    Int(Box<Expr>),
    Ln(Box<Expr>),
    Log10(Box<Expr>),
    Max(Box<Expr>, Box<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Pi,
    Pulse(Box<Expr>, Box<Expr>, Box<Expr>),
    SafeDiv(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Sin(Box<Expr>),
    Sqrt(Box<Expr>),
    Tan(Box<Expr>),
}

#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(f64),
    Var(usize), // offset
    GlobalVar(Ident),
    App(BuiltinFn),
    EvalModule(Ident, Vec<Expr>),
    Op2(BinaryOp, Box<Expr>, Box<Expr>),
    Op1(UnaryOp, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
}

struct Context<'a> {
    ident: &'a str,
    offsets: &'a HashMap<&'a str, usize>,
    is_initial: bool,
}

impl<'a> Context<'a> {
    fn lower(&self, expr: &ast::Expr) -> Result<Expr> {
        let expr = match expr {
            ast::Expr::Const(_, n) => Expr::Const(*n),
            ast::Expr::Var(id) => Expr::Var(self.offsets[id.as_str()]),
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
                        if let ast::Expr::Var(ident) = (&orig_args[0]).as_ref() {
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
            .map(|flow| Expr::Var(self.offsets[flow.as_str()]));

        let first = loads.next().unwrap();
        Some(loads.fold(first, |acc, flow| {
            Expr::Op2(BinaryOp::Add, Box::new(acc), Box::new(flow))
        }))
    }

    fn build_stock_update_expr(&self, var: &Variable) -> Result<Expr> {
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

            Ok(Expr::Op2(
                BinaryOp::Sub,
                Box::new(inflows),
                Box::new(outflows),
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
        Rc::new(If(
            Rc::new(Op2(
                And,
                Rc::new(Var("true_input".to_string())),
                Rc::new(Var("false_input".to_string())),
            )),
            Rc::new(Const("1".to_string(), 1.0)),
            Rc::new(Const("0".to_string(), 0.0)),
        ))
    };

    let mut offsets: HashMap<&str, usize> = HashMap::new();
    offsets.insert("true_input", 7);
    offsets.insert("false_input", 8);
    let context = Context {
        ident: "test",
        offsets: &offsets,
        is_initial: false,
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
        Rc::new(If(
            Rc::new(Op2(
                Or,
                Rc::new(Var("true_input".to_string())),
                Rc::new(Var("false_input".to_string())),
            )),
            Rc::new(Const("1".to_string(), 1.0)),
            Rc::new(Const("0".to_string(), 0.0)),
        ))
    };

    let mut offsets: HashMap<&str, usize> = HashMap::new();
    offsets.insert("true_input", 7);
    offsets.insert("false_input", 8);
    let context = Context {
        ident: "test",
        offsets: &offsets,
        is_initial: false,
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
    off: usize,
    ast: Expr,
}

#[test]
fn test_fold_flows() {
    use std::iter::FromIterator;

    let offsets: &[(&str, usize)] = &[("time", 0), ("a", 1), ("b", 2), ("c", 3), ("d", 4)];
    let offsets: HashMap<&str, usize> =
        HashMap::from_iter(offsets.into_iter().map(|(k, v)| (*k, *v)));
    let ctx = Context {
        ident: "test",
        offsets: &offsets,
        is_initial: false,
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
        let off = ctx.offsets[var.ident().as_str()];
        let ast = match var {
            Variable::Module { ident, inputs, .. } => {
                let inputs: Vec<Expr> = inputs
                    .iter()
                    .map(|mi| Expr::GlobalVar(mi.src.clone()))
                    .collect();
                Expr::EvalModule(ident.clone(), inputs)
            }
            Variable::Stock { ast, .. } => {
                if ctx.is_initial {
                    if ast.is_none() {
                        return sim_err!(EmptyEquation, var.ident().clone());
                    }
                    ctx.lower(ast.as_ref().unwrap())?
                } else {
                    ctx.build_stock_update_expr(var)?
                }
            }
            Variable::Var {
                ident, table, ast, ..
            } => {
                if let Some(ast) = ast {
                    let expr = ctx.lower(ast)?;
                    if table.is_some() {
                        Expr::App(BuiltinFn::Lookup(ident.clone(), Box::new(expr)))
                    } else {
                        expr
                    }
                } else {
                    return sim_err!(EmptyEquation, var.ident().clone());
                }
            }
        };
        Ok(Var { off, ast })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    // inputs: Vec<f64>,
    base_off: usize, // base offset for this module
    n_slots: usize,  // number of f64s we need storage for
    runlist_initials: Vec<Var>,
    runlist_flows: Vec<Var>,
    runlist_stocks: Vec<Var>,
    offsets: HashMap<String, usize>,
    tables: HashMap<String, Table>,
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

fn calc_n_slots(project: &Project, model_name: &str) -> usize {
    let model = Rc::clone(&project.models[model_name]);

    model
        .variables
        .iter()
        .map(|(_name, var)| {
            if let Variable::Module { ident, .. } = var {
                calc_n_slots(project, ident)
            } else {
                1
            }
        })
        .sum()
}

impl Module {
    fn new(project: &Project, model: Rc<Model>, is_root: bool) -> Result<Self> {
        if model.dt_deps.is_none() || model.initial_deps.is_none() {
            return sim_err!(NotSimulatable, model.name.clone());
        }

        let model_name: &str = &model.name;
        let n_slots_start_off = if model_name == "main" { 1 } else { 0 };

        // FIXME: not right -- needs to adjust for submodules
        let n_slots = n_slots_start_off + calc_n_slots(project, model_name);

        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            // TODO: if we reorder based on dependencies, we could probably improve performance
            //   through better cache behavior.
            var_names.sort();
            var_names
        };

        let offsets: HashMap<&str, usize> = {
            let mut offsets = HashMap::new();
            let base: usize = if is_root {
                offsets.insert("time", 0);
                1
            } else {
                0
            };
            offsets.extend(
                var_names
                    .iter()
                    .enumerate()
                    .map(|(i, ident)| (*ident, base + i)),
            );

            offsets
        };

        let initial_deps = model.initial_deps.as_ref().unwrap();
        let is_initial = true;

        // TODO: we can cut this down to just things needed to initialize stocks,
        //   but thats just an optimization
        let runlist_initials: Vec<&str> = var_names.clone();
        let runlist_initials = topo_sort(&model.variables, initial_deps, runlist_initials);
        let runlist_initials: Result<Vec<Var>> = runlist_initials
            .into_iter()
            .map(|ident| {
                Var::new(
                    &Context {
                        ident,
                        offsets: &offsets,
                        is_initial,
                    },
                    &model.variables[ident],
                )
            })
            .collect();

        let dt_deps = model.dt_deps.as_ref().unwrap();
        let is_initial = false;

        let runlist_flows: Vec<&str> = var_names
            .iter()
            .cloned()
            .filter(|id| !(&model.variables[*id]).is_stock())
            .collect();
        let runlist_flows = topo_sort(&model.variables, dt_deps, runlist_flows);
        let runlist_flows: Result<Vec<Var>> = runlist_flows
            .into_iter()
            .map(|ident| {
                Var::new(
                    &Context {
                        ident,
                        offsets: &offsets,
                        is_initial,
                    },
                    &model.variables[ident],
                )
            })
            .collect();

        // no sorting needed for stocks
        let runlist_stocks: Result<Vec<Var>> = var_names
            .iter()
            .map(|id| &model.variables[*id])
            .filter(|v| v.is_stock())
            .map(|v| {
                Var::new(
                    &Context {
                        ident: v.ident(),
                        offsets: &offsets,
                        is_initial,
                    },
                    v,
                )
            })
            .collect();

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

        Ok(Module {
            base_off: 0,
            n_slots,
            runlist_initials: runlist_initials?,
            runlist_flows: runlist_flows?,
            runlist_stocks: runlist_stocks?,
            offsets: offsets
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            tables,
        })
    }
}

fn is_truthy(n: f64) -> bool {
    let is_false = approx_eq!(f64, n, 0.0);
    !is_false
}

pub struct StepEvaluator<'a> {
    curr: &'a [f64],
    off: usize,
    dt: f64,
    tables: &'a HashMap<String, Table>,
}

impl<'a> StepEvaluator<'a> {
    fn eval(&self, expr: &Expr) -> f64 {
        match expr {
            Expr::Const(n) => *n,
            Expr::GlobalVar(id) => 0.0,
            Expr::EvalModule(ident, args) => {
                let args: Vec<f64> = args.iter().map(|arg| self.eval(arg)).collect();

                0.0
            }
            Expr::Var(off) => self.curr[*off],
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
                        if !self.tables.contains_key(id) {
                            eprintln!("bad lookup for {}", id);
                            unreachable!();
                        }
                        let table = &self.tables[id].data;
                        if table.is_empty() {
                            return f64::NAN;
                        }

                        let index = self.eval(index);

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

                        let mut next_pulse = first_pulse;
                        while time >= next_pulse {
                            if time < next_pulse + self.dt {
                                return volume / self.dt;
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

#[derive(Debug)]
pub struct Results {
    pub offsets: HashMap<String, usize>,
    // one large allocation
    pub data: Box<[f64]>,
    pub step_size: usize,
    pub step_count: usize,
}

impl Results {
    pub fn print_tsv(&self) {
        let var_names = {
            let offset_name_map: HashMap<usize, &str> =
                self.offsets.iter().map(|(k, v)| (*v, k.as_str())).collect();
            let mut var_names: Vec<&str> = Vec::with_capacity(self.step_size);
            for i in 0..(self.step_size) {
                var_names.push(offset_name_map[&i]);
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
    modules: Vec<Module>,
    specs: Specs,
    root: usize, // offset into modules
}

impl Simulation {
    pub fn new(project: &Project, model: Rc<Model>) -> Result<Self> {
        // we start with a project and a root module (one with no references).
        let root = Module::new(project, model, true)?;

        // TODO: come up with monomorphizations based on what inputs are used

        // module assign offsets

        // reset

        let specs = Specs::from(project.file.sim_specs.as_ref().unwrap());

        Ok(Simulation {
            modules: vec![root],
            specs,
            root: 0,
        })
    }

    fn calc_initials(&self, module_id: usize, dt: f64, curr: &mut [f64]) {
        let module = &self.modules[module_id];
        curr[TIME_OFF] = self.specs.start;

        for v in module.runlist_initials.iter() {
            curr[v.off] = StepEvaluator {
                dt,
                off: 0,
                curr,
                tables: &module.tables,
            }
            .eval(&v.ast);
        }
    }

    fn calc_flows(
        &self,
        offsets: &HashMap<Ident, usize>,
        module_id: usize,
        dt: f64,
        curr: &mut [f64],
    ) {
        let module = &self.modules[module_id];
        for v in module.runlist_flows.iter() {
            curr[v.off] = StepEvaluator {
                dt,
                off: 0,
                curr,
                tables: &module.tables,
            }
            .eval(&v.ast);
        }
    }

    fn calc_stocks(
        &self,
        offsets: &HashMap<Ident, usize>,
        module_id: usize,
        dt: f64,
        curr: &[f64],
        next: &mut [f64],
    ) {
        let module = &self.modules[module_id];
        for v in module.runlist_stocks.iter() {
            next[v.off] = curr[v.off]
                + StepEvaluator {
                    dt,
                    off: 0,
                    curr,
                    tables: &module.tables,
                }
                .eval(&v.ast)
                    * dt;
        }
    }

    fn n_slots(&self, module_id: usize) -> usize {
        self.modules[module_id].n_slots
    }

    fn build_offsets(&self, module_id: usize, _prefix: &str) -> HashMap<String, usize> {
        self.modules[module_id].offsets.clone()
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
        let save_every = std::cmp::max(1, (spec.save_step / spec.dt + 0.5) as usize);

        let dt = spec.dt;
        let stop = spec.stop;

        let n_slots = self.n_slots(self.root);

        let offsets = self.build_offsets(self.root, "");

        let slab: Vec<f64> = vec![0.0; n_slots * (n_chunks + 1)];
        let mut boxed_slab = slab.into_boxed_slice();
        {
            let mut slabs = boxed_slab.chunks_mut(n_slots);

            // let mut results: Vec<&[f64]> = Vec::with_capacity(n_chunks + 1);
            let module_id = self.root;

            let mut curr = slabs.next().unwrap();
            self.calc_initials(module_id, dt, curr);

            let mut step = 0;
            let mut next = slabs.next().unwrap();
            loop {
                self.calc_flows(&offsets, module_id, dt, curr);
                self.calc_stocks(&offsets, module_id, dt, curr, next);
                next[TIME_OFF] = curr[TIME_OFF] + dt;
                step += 1;
                if step != save_every {
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
                }
            }
            // ensure we've calculated stock + flow values for the dt <= end_time
            assert!(curr[TIME_OFF] > stop);
        }

        Ok(Results {
            offsets,
            data: boxed_slab,
            step_size: n_slots,
            step_count: n_chunks,
        })
    }
}
