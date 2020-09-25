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
    Dt,
    App(BuiltinFn),
    EvalModule(Ident, Vec<Expr>),
    ModuleInput(usize),
    Op2(BinaryOp, Box<Expr>, Box<Expr>),
    Op1(UnaryOp, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    AssignCurr(usize, Box<Expr>),
    AssignNext(usize, Box<Expr>),
}

struct Context<'a> {
    ident: &'a str,
    offsets: &'a HashMap<String, usize>,
    is_initial: bool,
    inputs: &'a [Ident],
}

impl<'a> Context<'a> {
    fn get_offset(&self, ident: &str) -> Result<usize> {
        if self.offsets.contains_key(ident) {
            Ok(self.offsets[ident])
        } else {
            unreachable!();
        }
    }

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

    let inputs = &[];
    let mut offsets: HashMap<String, usize> = HashMap::new();
    offsets.insert("true_input".to_string(), 7);
    offsets.insert("false_input".to_string(), 8);
    let context = Context {
        ident: "test",
        offsets: &offsets,
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

    let inputs = &[];
    let mut offsets: HashMap<String, usize> = HashMap::new();
    offsets.insert("true_input".to_string(), 7);
    offsets.insert("false_input".to_string(), 8);
    let context = Context {
        ident: "test",
        offsets: &offsets,
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
    ast: Expr,
}

#[test]
fn test_fold_flows() {
    use std::iter::FromIterator;

    let inputs = &[];
    let offsets: &[(&str, usize)] = &[("time", 0), ("a", 1), ("b", 2), ("c", 3), ("d", 4)];
    let offsets: HashMap<String, usize> =
        HashMap::from_iter(offsets.into_iter().map(|(k, v)| ((*k).to_string(), *v)));
    let ctx = Context {
        ident: "test",
        offsets: &offsets,
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
        let ast = if let Some((off, _)) = ctx
            .inputs
            .iter()
            .enumerate()
            .find(|(_i, n)| *n == var.ident())
        {
            Expr::AssignCurr(off, Box::new(Expr::ModuleInput(off)))
        } else {
            match var {
                Variable::Module { ident, inputs, .. } => {
                    let inputs: Vec<Expr> = inputs
                        .iter()
                        .map(|mi| Expr::Var(ctx.get_offset(&mi.src).unwrap()))
                        .collect();
                    Expr::EvalModule(ident.clone(), inputs)
                }
                Variable::Stock { ast, .. } => {
                    let off = ctx.offsets[var.ident()];
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        Expr::AssignCurr(off, Box::new(ctx.lower(ast.as_ref().unwrap())?))
                    } else {
                        Expr::AssignNext(off, Box::new(ctx.build_stock_update_expr(off, var)?))
                    }
                }
                Variable::Var {
                    ident, table, ast, ..
                } => {
                    let off = ctx.offsets[var.ident()];
                    if let Some(ast) = ast {
                        let expr = ctx.lower(ast)?;
                        let expr = if table.is_some() {
                            Expr::App(BuiltinFn::Lookup(ident.clone(), Box::new(expr)))
                        } else {
                            expr
                        };
                        Expr::AssignCurr(off, Box::new(expr))
                    } else {
                        return sim_err!(EmptyEquation, var.ident().to_string());
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

fn calc_offsets(project: &Project, model_name: &str) -> HashMap<Ident, usize> {
    let is_root = model_name == "main";

    let mut offsets: HashMap<Ident, usize> = HashMap::new();
    let mut base = 0;
    if is_root {
        offsets.insert("time".to_string(), 0);
        base += 1;
    }

    let model = Rc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        // TODO: if we reorder based on dependencies, we could probably improve performance
        //   through better cache behavior.
        var_names.sort();
        var_names
    };

    for (i, ident) in var_names.iter().enumerate() {
        if let Variable::Module { .. } = &model.variables[*ident] {
            let sub_offsets = calc_offsets(project, *ident);
            let mut sub_var_names: Vec<&str> = sub_offsets.keys().map(|v| v.as_str()).collect();
            sub_var_names.sort();
            for (j, sub_ident) in sub_var_names.iter().enumerate() {
                offsets.insert(format!("{}.{}", *ident, sub_ident), base + i + j);
            }
            // so we can find the module offset when evaling the submodule
            offsets.insert(ident.to_string(), base + i);
            // TODO: -1 because we didn't use the "slot" reserved for the
            //   module in the parent model
            base += sub_offsets.len() - 1;
        } else {
            offsets.insert(ident.to_string(), base + i);
        }
    }

    offsets
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
    fn new(project: &Project, model: Rc<Model>, inputs: &[Ident], is_root: bool) -> Result<Self> {
        if model.dt_deps.is_none() || model.initial_deps.is_none() {
            return sim_err!(NotSimulatable, model.name.clone());
        }

        let model_name: &str = &model.name;
        let n_slots_start_off = if is_root { 1 } else { 0 };
        let n_slots = n_slots_start_off + calc_n_slots(project, model_name);
        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            // TODO: if we reorder based on dependencies, we could probably improve performance
            //   through better cache behavior.
            var_names.sort();
            var_names
        };

        let offsets = calc_offsets(project, model_name);

        let build_runlist = |deps: &HashMap<Ident, HashSet<Ident>>,
                             part: StepPart,
                             predicate: &dyn Fn(&&str) -> bool|
         -> Result<Vec<Var>> {
            let runlist: Vec<&str> = var_names.iter().cloned().filter(predicate).collect();
            let runlist = match part {
                StepPart::Initials | StepPart::Flows => topo_sort(&model.variables, deps, runlist),
                StepPart::Stocks => runlist,
            };
            eprintln!("runlist {}", model_name);
            for (i, name) in runlist.iter().enumerate() {
                eprintln!("  {}: {}", i, name);
            }
            let is_initial = match part {
                StepPart::Initials => true,
                _ => false,
            };
            let runlist: Result<Vec<Var>> = runlist
                .into_iter()
                .map(|ident| {
                    Var::new(
                        &Context {
                            ident,
                            offsets: &offsets,
                            is_initial,
                            inputs,
                        },
                        &model.variables[ident],
                    )
                })
                .collect();
            for v in runlist.clone().unwrap().iter() {
                eprintln!("{}", pretty(&v.ast));
            }

            runlist
        };

        let initial_deps = model.initial_deps.as_ref().unwrap();
        // TODO: we can cut this down to just things needed to initialize stocks,
        //   but thats just an optimization
        let runlist_initials = build_runlist(initial_deps, StepPart::Initials, &|_| true)?;

        let dt_deps = model.dt_deps.as_ref().unwrap();
        let runlist_flows = build_runlist(dt_deps, StepPart::Flows, &|id| {
            !(&model.variables[*id]).is_stock()
        });
        let runlist_stocks = build_runlist(dt_deps, StepPart::Stocks, &|id| {
            (&model.variables[*id]).is_stock()
        });

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
            ident: model_name.to_string(),
            n_slots,
            runlist_initials,
            runlist_flows: runlist_flows?,
            runlist_stocks: runlist_stocks?,
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
    dt: f64,
    module: &'a Module,
    modules: &'a HashMap<&'a str, &'a Module>,
    sim: &'a Simulation,
}

impl<'a> ModuleEvaluator<'a> {
    fn eval(&mut self, expr: &Expr) -> f64 {
        match expr {
            Expr::Const(n) => *n,
            Expr::Dt => self.dt,
            Expr::ModuleInput(off) => self.inputs[*off],
            Expr::EvalModule(ident, args) => {
                let args: Vec<f64> = args.iter().map(|arg| self.eval(arg)).collect();
                let off = self.off + self.module.offsets[ident];
                let module = self.modules[ident.as_str()];

                self.sim.calc(
                    self.step_part,
                    self.modules,
                    module,
                    off,
                    &args,
                    self.dt,
                    self.curr,
                    self.next,
                );

                0.0
            }
            Expr::Var(off) => self.curr[*off],
            Expr::AssignCurr(off, r) => {
                self.curr[self.off + *off] = self.eval(r);
                0.0
            }
            Expr::AssignNext(off, r) => {
                self.next[self.off + *off] = self.eval(r);
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

fn pretty(expr: &Expr) -> String {
    match expr {
        Expr::Const(n) => format!("{}", n),
        Expr::Var(off) => format!("curr[{}]", off),
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
        Expr::EvalModule(module, args) => {
            let args: Vec<_> = args.iter().map(|arg| pretty(arg)).collect();
            let string_args = args.join(", ");
            format!("eval<{}>({})", module, string_args)
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
    project: Rc<Project>,
}

fn enumerate_modules(
    project: &Project,
    model_name: &str,
    modules: &mut HashSet<(Ident, Vec<Ident>)>,
) -> Result<()> {
    use crate::common::{Error, ErrorCode};
    let model = project.models.get(model_name).ok_or_else(|| {
        Error::SimulationError(
            ErrorCode::NotSimulatable,
            format!("model for module '{}' not found", model_name),
        )
    })?;
    let model = Rc::clone(model);
    for (id, v) in model.variables.iter() {
        if let Variable::Module { inputs, .. } = v {
            let mut inputs: Vec<String> = inputs.iter().map(|input| input.dst.clone()).collect();
            inputs.sort();
            if modules.insert((id.to_string(), inputs)) {
                // first time we're seeing this monomorphization; recurse
                enumerate_modules(project, id.as_str(), modules)?;
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
            module_names.sort();

            let mut sorted_names = vec![main_model_name];
            sorted_names.extend(module_names.into_iter().filter(|n| *n != main_model_name));
            sorted_names
        };

        let mut compiled_modules: Vec<Module> = Vec::new();
        for name in module_names {
            for (_, inputs) in modules.iter().filter(|(n, _)| n == name) {
                let model = Rc::clone(&project.models[name]);
                let is_root = name == main_model_name;
                let module = Module::new(project, model, inputs, is_root)?;
                compiled_modules.push(module);
            }
        }

        // module assign offsets

        // reset

        let specs = Specs::from(project.file.sim_specs.as_ref().unwrap());

        Ok(Simulation {
            modules: compiled_modules,
            specs,
            root: 0,
            project: Rc::clone(project_rc),
        })
    }

    fn calc(
        &self,
        step_part: StepPart,
        modules: &HashMap<&str, &Module>,
        module: &Module,
        module_off: usize,
        module_inputs: &[f64],
        dt: f64,
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
            dt,
            off: module_off,
            curr,
            next,
            module,
            modules,
            inputs: module_inputs,
            sim: self,
        };

        for v in runlist.iter() {
            step.eval(&v.ast);
        }
    }

    fn n_slots(&self, module_id: usize) -> usize {
        self.modules[module_id].n_slots
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

        let module = &self.modules[self.root];
        let modules: HashMap<&str, &Module> =
            self.modules.iter().map(|m| (m.ident.as_str(), m)).collect();

        let slab: Vec<f64> = vec![0.0; n_slots * (n_chunks + 1)];
        let mut boxed_slab = slab.into_boxed_slice();
        {
            let mut slabs = boxed_slab.chunks_mut(n_slots);

            // let mut results: Vec<&[f64]> = Vec::with_capacity(n_chunks + 1);

            let module_inputs: &[f64] = &[];

            let mut curr = slabs.next().unwrap();
            let mut next = slabs.next().unwrap();
            curr[TIME_OFF] = self.specs.start;
            self.calc(
                StepPart::Initials,
                &modules,
                module,
                0,
                module_inputs,
                dt,
                curr,
                next,
            );

            let mut step = 0;
            loop {
                self.calc(
                    StepPart::Flows,
                    &modules,
                    module,
                    0,
                    module_inputs,
                    dt,
                    curr,
                    next,
                );
                self.calc(
                    StepPart::Stocks,
                    &modules,
                    module,
                    0,
                    module_inputs,
                    dt,
                    curr,
                    next,
                );
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

        let offsets = calc_offsets(&self.project, &module.ident);

        Ok(Results {
            offsets,
            data: boxed_slab,
            step_size: n_slots,
            step_count: n_chunks,
        })
    }
}
