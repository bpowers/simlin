// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::borrow::BorrowMut;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use float_cmp::approx_eq;

use crate::ast::{self, Ast, BinaryOp, IndexExpr, Loc};
use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeBuilder, ByteCodeContext, CompiledModule, GraphicalFunctionId,
    ModuleDeclaration, ModuleId, ModuleInputOffset, Op2, Opcode, VariableOffset,
};
use crate::common::{quoteize, ErrorCode, ErrorKind, Ident, Result};
use crate::datamodel::{self, Dimension};
use crate::interpreter::UnaryOp;
use crate::model::{enumerate_modules, ModelStage1};
use crate::project::Project;
use crate::variable::Variable;
use crate::vm::{
    is_truthy, pulse, ramp, step, CompiledSimulation, Results, Specs, StepPart, SubscriptIterator,
    DT_OFF, FINAL_TIME_OFF, IMPLICIT_VAR_COUNT, INITIAL_TIME_OFF, TIME_OFF,
};
use crate::{sim_err, Error};

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

type BuiltinFn = crate::builtins::BuiltinFn<Expr>;

#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(f64, Loc),
    Var(usize, Loc),                              // offset
    Subscript(usize, Vec<Expr>, Vec<usize>, Loc), // offset, index expression, bounds
    Dt(Loc),
    App(BuiltinFn, Loc),
    EvalModule(Ident, Ident, Vec<Expr>),
    ModuleInput(usize, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, Loc),
    Op1(UnaryOp, Box<Expr>, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, Loc),
    AssignCurr(usize, Box<Expr>),
    AssignNext(usize, Box<Expr>),
}

impl Expr {
    fn get_loc(&self) -> Loc {
        match self {
            Expr::Const(_, loc) => *loc,
            Expr::Var(_, loc) => *loc,
            Expr::Subscript(_, _, _, loc) => *loc,
            Expr::Dt(loc) => *loc,
            Expr::App(_, loc) => *loc,
            Expr::EvalModule(_, _, _) => Loc::default(),
            Expr::ModuleInput(_, loc) => *loc,
            Expr::Op2(_, _, _, loc) => *loc,
            Expr::Op1(_, _, loc) => *loc,
            Expr::If(_, _, _, loc) => *loc,
            Expr::AssignCurr(_, _) => Loc::default(),
            Expr::AssignNext(_, _) => Loc::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr::Const(c, _loc) => Expr::Const(c, loc),
            Expr::Var(v, _loc) => Expr::Var(v, loc),
            Expr::Subscript(off, subscripts, bounds, _) => {
                let subscripts = subscripts
                    .into_iter()
                    .map(|expr| expr.strip_loc())
                    .collect();
                Expr::Subscript(off, subscripts, bounds, loc)
            }
            Expr::Dt(_) => Expr::Dt(loc),
            Expr::App(builtin, _loc) => {
                let builtin = match builtin {
                    // nothing to strip from these simple ones
                    BuiltinFn::Inf
                    | BuiltinFn::Pi
                    | BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => builtin,
                    BuiltinFn::IsModuleInput(id, _loc) => BuiltinFn::IsModuleInput(id, loc),
                    BuiltinFn::Lookup(id, a, _loc) => {
                        BuiltinFn::Lookup(id, Box::new(a.strip_loc()), loc)
                    }
                    BuiltinFn::Abs(a) => BuiltinFn::Abs(Box::new(a.strip_loc())),
                    BuiltinFn::Arccos(a) => BuiltinFn::Arccos(Box::new(a.strip_loc())),
                    BuiltinFn::Arcsin(a) => BuiltinFn::Arcsin(Box::new(a.strip_loc())),
                    BuiltinFn::Arctan(a) => BuiltinFn::Arctan(Box::new(a.strip_loc())),
                    BuiltinFn::Cos(a) => BuiltinFn::Cos(Box::new(a.strip_loc())),
                    BuiltinFn::Exp(a) => BuiltinFn::Exp(Box::new(a.strip_loc())),
                    BuiltinFn::Int(a) => BuiltinFn::Int(Box::new(a.strip_loc())),
                    BuiltinFn::Ln(a) => BuiltinFn::Ln(Box::new(a.strip_loc())),
                    BuiltinFn::Log10(a) => BuiltinFn::Log10(Box::new(a.strip_loc())),
                    BuiltinFn::Mean(args) => {
                        BuiltinFn::Mean(args.into_iter().map(|arg| arg.strip_loc()).collect())
                    }
                    BuiltinFn::Sin(a) => BuiltinFn::Sin(Box::new(a.strip_loc())),
                    BuiltinFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(a.strip_loc())),
                    BuiltinFn::Tan(a) => BuiltinFn::Tan(Box::new(a.strip_loc())),
                    BuiltinFn::Max(a, b) => {
                        BuiltinFn::Max(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::Min(a, b) => {
                        BuiltinFn::Min(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::Step(a, b) => {
                        BuiltinFn::Step(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::Pulse(a, b, c) => BuiltinFn::Pulse(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::Ramp(a, b, c) => BuiltinFn::Ramp(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::SafeDiv(a, b, c) => BuiltinFn::SafeDiv(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                };
                Expr::App(builtin, loc)
            }
            Expr::EvalModule(id1, id2, args) => {
                let args = args.into_iter().map(|expr| expr.strip_loc()).collect();
                Expr::EvalModule(id1, id2, args)
            }
            Expr::ModuleInput(mi, _loc) => Expr::ModuleInput(mi, loc),
            Expr::Op2(op, l, r, _loc) => {
                Expr::Op2(op, Box::new(l.strip_loc()), Box::new(r.strip_loc()), loc)
            }
            Expr::Op1(op, r, _loc) => Expr::Op1(op, Box::new(r.strip_loc()), loc),
            Expr::If(cond, t, f, _loc) => Expr::If(
                Box::new(cond.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                loc,
            ),
            Expr::AssignCurr(off, rhs) => Expr::AssignCurr(off, Box::new(rhs.strip_loc())),
            Expr::AssignNext(off, rhs) => Expr::AssignNext(off, Box::new(rhs.strip_loc())),
        }
    }
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
    #[allow(dead_code)]
    ident: &'a str,
    active_dimension: Option<Vec<datamodel::Dimension>>,
    active_subscript: Option<Vec<&'a str>>,
    metadata: &'a HashMap<Ident, HashMap<Ident, VariableMetadata>>,
    module_models: &'a HashMap<Ident, HashMap<Ident, Ident>>,
    is_initial: bool,
    inputs: &'a BTreeSet<Ident>,
}

impl<'a> Context<'a> {
    fn get_offset(&self, ident: &str) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, false)
    }

    /// get_base_offset ignores arrays and should only be used from Var::new and Expr::Subscript
    fn get_base_offset(&self, ident: &str) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, true)
    }

    fn get_metadata(&self, ident: &str) -> Result<&VariableMetadata> {
        self.get_submodel_metadata(self.model_name, ident)
    }

    fn get_implicit_subscripts(&self, dims: &[Dimension], ident: &str) -> Result<Vec<&str>> {
        if self.active_dimension.is_none() {
            return sim_err!(ArrayReferenceNeedsExplicitSubscripts, ident.to_owned());
        }
        let active_dims = self.active_dimension.as_ref().unwrap();
        let active_subscripts = self.active_subscript.as_ref().unwrap();
        assert_eq!(active_dims.len(), active_subscripts.len());

        // if we need more dimensions than are implicit, that's an error
        if dims.len() > active_dims.len() {
            return sim_err!(MismatchedDimensions, ident.to_owned());
        }

        // goal: if this is a valid equation, dims will be a subset of active_dims (order preserving)

        let mut subscripts: Vec<&str> = Vec::with_capacity(dims.len());

        let mut active_off = 0;
        for dim in dims.iter() {
            while active_off < active_dims.len() {
                let off = active_off;
                active_off += 1;
                let candidate = &active_dims[off];
                if candidate.name() == dim.name() {
                    subscripts.push(active_subscripts[off]);
                    break;
                }
            }
        }

        if subscripts.len() != dims.len() {
            return sim_err!(MismatchedDimensions, ident.to_owned());
        }

        Ok(subscripts)
    }

    fn get_implicit_subscript_off(&self, dims: &[Dimension], ident: &str) -> Result<usize> {
        let subscripts = self.get_implicit_subscripts(dims, ident)?;

        let off = dims
            .iter()
            .zip(subscripts)
            .fold(0_usize, |acc, (dim, subscript)| {
                acc * dim.len() + dim.get_offset(subscript).unwrap()
            });

        Ok(off)
    }

    fn get_dimension_name_subscript(&self, dim_name: &str) -> Option<usize> {
        let active_dims = self.active_dimension.as_ref()?;
        let active_subscripts = self.active_subscript.as_ref().unwrap();

        for (dim, subscript) in active_dims.iter().zip(active_subscripts) {
            if dim.name() == dim_name {
                return dim.get_offset(subscript);
            }
        }

        None
    }

    fn get_submodel_metadata(&self, model: &str, ident: &str) -> Result<&VariableMetadata> {
        let metadata = &self.metadata[model];
        if let Some(pos) = ident.find('·') {
            let submodel_module_name = &ident[..pos];
            let submodel_name = &self.module_models[model][submodel_module_name];
            let submodel_var = &ident[pos + '·'.len_utf8()..];
            self.get_submodel_metadata(submodel_name, submodel_var)
        } else {
            Ok(&metadata[ident])
        }
    }

    fn get_submodel_offset(&self, model: &str, ident: &str, ignore_arrays: bool) -> Result<usize> {
        let metadata = &self.metadata[model];
        if let Some(pos) = ident.find('·') {
            let submodel_module_name = &ident[..pos];
            let submodel_name = &self.module_models[model][submodel_module_name];
            let submodel_var = &ident[pos + '·'.len_utf8()..];
            let submodel_off = metadata[submodel_module_name].offset;
            Ok(submodel_off
                + self.get_submodel_offset(submodel_name, submodel_var, ignore_arrays)?)
        } else if !ignore_arrays {
            if !metadata.contains_key(ident) {
                return sim_err!(DoesNotExist);
            }
            if let Some(dims) = metadata[ident].var.get_dimensions() {
                let off = self.get_implicit_subscript_off(dims, ident)?;
                Ok(metadata[ident].offset + off)
            } else {
                Ok(metadata[ident].offset)
            }
        } else {
            Ok(metadata[ident].offset)
        }
    }

    fn lower(&self, expr: &ast::Expr) -> Result<Expr> {
        let expr = match expr {
            ast::Expr::Const(_, n, loc) => Expr::Const(*n, *loc),
            ast::Expr::Var(id, loc) => {
                if let Some((off, _)) = self
                    .inputs
                    .iter()
                    .enumerate()
                    .find(|(_, input)| id == *input)
                {
                    Expr::ModuleInput(off, *loc)
                } else {
                    match self.get_offset(id) {
                        Ok(off) => Expr::Var(off, *loc),
                        Err(err) => {
                            return Err(err);
                        }
                    }
                }
            }
            ast::Expr::App(builtin, loc) => {
                use crate::builtins::BuiltinFn as BFn;
                let builtin: BuiltinFn = match builtin {
                    BFn::Lookup(id, expr, loc) => {
                        BuiltinFn::Lookup(id.clone(), Box::new(self.lower(expr)?), *loc)
                    }
                    BFn::Abs(a) => BuiltinFn::Abs(Box::new(self.lower(a)?)),
                    BFn::Arccos(a) => BuiltinFn::Arccos(Box::new(self.lower(a)?)),
                    BFn::Arcsin(a) => BuiltinFn::Arcsin(Box::new(self.lower(a)?)),
                    BFn::Arctan(a) => BuiltinFn::Arctan(Box::new(self.lower(a)?)),
                    BFn::Cos(a) => BuiltinFn::Cos(Box::new(self.lower(a)?)),
                    BFn::Exp(a) => BuiltinFn::Exp(Box::new(self.lower(a)?)),
                    BFn::Inf => BuiltinFn::Inf,
                    BFn::Int(a) => BuiltinFn::Int(Box::new(self.lower(a)?)),
                    BFn::IsModuleInput(id, loc) => BuiltinFn::IsModuleInput(id.clone(), *loc),
                    BFn::Ln(a) => BuiltinFn::Ln(Box::new(self.lower(a)?)),
                    BFn::Log10(a) => BuiltinFn::Log10(Box::new(self.lower(a)?)),
                    BFn::Max(a, b) => {
                        BuiltinFn::Max(Box::new(self.lower(a)?), Box::new(self.lower(b)?))
                    }
                    BFn::Mean(args) => {
                        let args = args
                            .iter()
                            .map(|arg| self.lower(arg))
                            .collect::<Result<Vec<Expr>>>();
                        BuiltinFn::Mean(args?)
                    }
                    BFn::Min(a, b) => {
                        BuiltinFn::Min(Box::new(self.lower(a)?), Box::new(self.lower(b)?))
                    }
                    BFn::Pi => BuiltinFn::Pi,
                    BFn::Pulse(a, b, c) => {
                        let c = match c {
                            Some(c) => Some(Box::new(self.lower(c)?)),
                            None => None,
                        };
                        BuiltinFn::Pulse(Box::new(self.lower(a)?), Box::new(self.lower(b)?), c)
                    }
                    BFn::Ramp(a, b, c) => {
                        let c = match c {
                            Some(c) => Some(Box::new(self.lower(c)?)),
                            None => None,
                        };
                        BuiltinFn::Ramp(Box::new(self.lower(a)?), Box::new(self.lower(b)?), c)
                    }
                    BFn::SafeDiv(a, b, c) => {
                        let c = match c {
                            Some(c) => Some(Box::new(self.lower(c)?)),
                            None => None,
                        };
                        BuiltinFn::SafeDiv(Box::new(self.lower(a)?), Box::new(self.lower(b)?), c)
                    }
                    BFn::Sin(a) => BuiltinFn::Sin(Box::new(self.lower(a)?)),
                    BFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(self.lower(a)?)),
                    BFn::Step(a, b) => {
                        BuiltinFn::Step(Box::new(self.lower(a)?), Box::new(self.lower(b)?))
                    }
                    BFn::Tan(a) => BuiltinFn::Tan(Box::new(self.lower(a)?)),
                    BFn::Time => BuiltinFn::Time,
                    BFn::TimeStep => BuiltinFn::TimeStep,
                    BFn::StartTime => BuiltinFn::StartTime,
                    BFn::FinalTime => BuiltinFn::FinalTime,
                };
                Expr::App(builtin, *loc)
            }
            ast::Expr::Subscript(id, args, loc) => {
                let off = self.get_base_offset(id)?;
                let metadata = self.get_metadata(id)?;
                let dims = metadata.var.get_dimensions().unwrap();
                if args.len() != dims.len() {
                    return sim_err!(MismatchedDimensions, id.clone());
                }
                let args: Result<Vec<_>> = args
                    .iter()
                    .enumerate()
                    .map(|(i, arg)| {
                        match arg {
                            IndexExpr::Wildcard(_loc) => sim_err!(TodoWildcard, id.clone()),
                            IndexExpr::StarRange(_id, _loc) => sim_err!(TodoStarRange, id.clone()),
                            IndexExpr::Range(_l, _r, _loc) => sim_err!(TodoRange, id.clone()),
                            IndexExpr::Expr(arg) => {
                                let expr = if let ast::Expr::Var(ident, loc) = arg {
                                    let dim = &dims[i];
                                    // we need to check to make sure that any explicit subscript names are
                                    // converted to offsets here and not passed to self.lower
                                    if let Some(subscript_off) = dim.get_offset(ident) {
                                        Expr::Const((subscript_off + 1) as f64, *loc)
                                    } else if let Some(subscript_off) =
                                        self.get_dimension_name_subscript(ident)
                                    {
                                        // some modelers do `Variable[SubscriptName]` in their A2A equations
                                        Expr::Const((subscript_off + 1) as f64, *loc)
                                    } else {
                                        self.lower(arg)?
                                    }
                                } else {
                                    self.lower(arg)?
                                };
                                Ok(expr)
                            }
                        }
                    })
                    .collect();
                let bounds = dims.iter().map(|dim| dim.len()).collect();
                Expr::Subscript(off, args?, bounds, *loc)
            }
            ast::Expr::Op1(op, l, loc) => {
                let l = self.lower(l)?;
                match op {
                    ast::UnaryOp::Negative => Expr::Op2(
                        BinaryOp::Sub,
                        Box::new(Expr::Const(0.0, *loc)),
                        Box::new(l),
                        *loc,
                    ),
                    ast::UnaryOp::Positive => l,
                    ast::UnaryOp::Not => Expr::Op1(UnaryOp::Not, Box::new(l), *loc),
                }
            }
            ast::Expr::Op2(op, l, r, loc) => {
                let l = self.lower(l)?;
                let r = self.lower(r)?;
                let op = match op {
                    ast::BinaryOp::Add => BinaryOp::Add,
                    ast::BinaryOp::Sub => BinaryOp::Sub,
                    ast::BinaryOp::Exp => BinaryOp::Exp,
                    ast::BinaryOp::Mul => BinaryOp::Mul,
                    ast::BinaryOp::Div => BinaryOp::Div,
                    ast::BinaryOp::Mod => BinaryOp::Mod,
                    ast::BinaryOp::Gt => BinaryOp::Gt,
                    ast::BinaryOp::Gte => BinaryOp::Gte,
                    ast::BinaryOp::Lt => BinaryOp::Lt,
                    ast::BinaryOp::Lte => BinaryOp::Lte,
                    ast::BinaryOp::Eq => BinaryOp::Eq,
                    ast::BinaryOp::Neq => BinaryOp::Neq,
                    ast::BinaryOp::And => BinaryOp::And,
                    ast::BinaryOp::Or => BinaryOp::Or,
                };
                Expr::Op2(op, Box::new(l), Box::new(r), *loc)
            }
            ast::Expr::If(cond, t, f, loc) => {
                let cond = self.lower(cond)?;
                let t = self.lower(t)?;
                let f = self.lower(f)?;
                Expr::If(Box::new(cond), Box::new(t), Box::new(f), *loc)
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
            .map(|flow| Expr::Var(self.get_offset(flow).unwrap(), Loc::default()));

        let first = loads.next().unwrap();
        Some(loads.fold(first, |acc, flow| {
            Expr::Op2(BinaryOp::Add, Box::new(acc), Box::new(flow), Loc::default())
        }))
    }

    fn build_stock_update_expr(&self, stock_off: usize, var: &Variable) -> Expr {
        if let Variable::Stock {
            inflows, outflows, ..
        } = var
        {
            // TODO: simplify the expressions we generate
            let inflows = match self.fold_flows(inflows) {
                None => Expr::Const(0.0, Loc::default()),
                Some(flows) => flows,
            };
            let outflows = match self.fold_flows(outflows) {
                None => Expr::Const(0.0, Loc::default()),
                Some(flows) => flows,
            };

            let dt_update = Expr::Op2(
                BinaryOp::Mul,
                Box::new(Expr::Op2(
                    BinaryOp::Sub,
                    Box::new(inflows),
                    Box::new(outflows),
                    Loc::default(),
                )),
                Box::new(Expr::Dt(Loc::default())),
                Loc::default(),
            );

            Expr::Op2(
                BinaryOp::Add,
                Box::new(Expr::Var(stock_off, Loc::default())),
                Box::new(dt_update),
                Loc::default(),
            )
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
                Box::new(Var("true_input".to_string(), Loc::default())),
                Box::new(Var("false_input".to_string(), Loc::default())),
                Loc::default(),
            )),
            Box::new(Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Const("0".to_string(), 0.0, Loc::default())),
            Loc::default(),
        ))
    };

    let inputs = &BTreeSet::new();
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
            Box::new(Expr::Var(7, Loc::default())),
            Box::new(Expr::Var(8, Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr::Const(1.0, Loc::default())),
        Box::new(Expr::Const(0.0, Loc::default())),
        Loc::default(),
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
                Box::new(Var("true_input".to_string(), Loc::default())),
                Box::new(Var("false_input".to_string(), Loc::default())),
                Loc::default(),
            )),
            Box::new(Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Const("0".to_string(), 0.0, Loc::default())),
            Loc::default(),
        ))
    };

    let inputs = &BTreeSet::new();
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
            Box::new(Expr::Var(7, Loc::default())),
            Box::new(Expr::Var(8, Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr::Const(1.0, Loc::default())),
        Box::new(Expr::Const(0.0, Loc::default())),
        Loc::default(),
    );

    let output = context.lower(&input);
    assert!(output.is_ok());
    assert_eq!(expected, output.unwrap());
}

#[derive(Clone, Debug, PartialEq)]
pub struct Var {
    ident: Ident,
    ast: Vec<Expr>,
}

#[test]
fn test_fold_flows() {
    let inputs = &BTreeSet::new();
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
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
    assert_eq!(
        Some(Expr::Var(1, Loc::default())),
        ctx.fold_flows(&["a".to_string()])
    );
    assert_eq!(
        Some(Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var(1, Loc::default())),
            Box::new(Expr::Var(4, Loc::default())),
            Loc::default(),
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
                Box::new(Expr::ModuleInput(off, Loc::default())),
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
                        .map(|mi| Expr::Var(ctx.get_offset(&mi.src).unwrap(), Loc::default()))
                        .collect();
                    vec![Expr::EvalModule(ident.clone(), model_name.clone(), inputs)]
                }
                Variable::Stock { init_ast: ast, .. } => {
                    let off = ctx.get_base_offset(var.ident())?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(ast) => {
                                vec![Expr::AssignCurr(off, Box::new(ctx.lower(ast)?))]
                            }
                            Ast::ApplyToAll(dims, ast) => {
                                let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(subscripts);
                                        ctx.lower(ast)
                                            .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                    })
                                    .collect();
                                exprs?
                            }
                            Ast::Arrayed(dims, elements) => {
                                let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let subscript_str = subscripts.join(",");
                                        let ast = &elements[&subscript_str];
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(subscripts);
                                        ctx.lower(ast)
                                            .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    } else {
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(_) => vec![Expr::AssignNext(
                                off,
                                Box::new(ctx.build_stock_update_expr(off, var)),
                            )],
                            Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _) => {
                                let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(subscripts);
                                        // when building the stock update expression, we need
                                        // the specific index of this subscript, not the base offset
                                        let update_expr = ctx.build_stock_update_expr(
                                            ctx.get_offset(var.ident())?,
                                            var,
                                        );
                                        Ok(Expr::AssignNext(off + i, Box::new(update_expr)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    }
                }
                Variable::Var { ident, table, .. } => {
                    let off = ctx.get_base_offset(var.ident())?;
                    let ast = if ctx.is_initial {
                        var.init_ast()
                    } else {
                        var.ast()
                    };
                    if ast.is_none() {
                        return sim_err!(EmptyEquation, var.ident().to_string());
                    }
                    match ast.as_ref().unwrap() {
                        Ast::Scalar(ast) => {
                            let expr = ctx.lower(ast)?;
                            let expr = if table.is_some() {
                                let loc = expr.get_loc();
                                Expr::App(
                                    BuiltinFn::Lookup(ident.clone(), Box::new(expr), loc),
                                    loc,
                                )
                            } else {
                                expr
                            };
                            vec![Expr::AssignCurr(off, Box::new(expr))]
                        }
                        Ast::ApplyToAll(dims, ast) => {
                            let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                .enumerate()
                                .map(|(i, subscripts)| {
                                    let mut ctx = ctx.clone();
                                    ctx.active_dimension = Some(dims.clone());
                                    ctx.active_subscript = Some(subscripts);
                                    ctx.lower(ast)
                                        .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                })
                                .collect();
                            exprs?
                        }
                        Ast::Arrayed(dims, elements) => {
                            let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                .enumerate()
                                .map(|(i, subscripts)| {
                                    let subscript_str = subscripts.join(",");
                                    let ast = &elements[&subscript_str];
                                    let mut ctx = ctx.clone();
                                    ctx.active_dimension = Some(dims.clone());
                                    ctx.active_subscript = Some(subscripts);
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
        Ok(Var {
            ident: var.ident().to_owned(),
            ast,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub(crate) ident: Ident,
    inputs: HashSet<Ident>,
    n_slots: usize, // number of f64s we need storage for
    pub(crate) runlist_initials: Vec<Expr>,
    pub(crate) runlist_flows: Vec<Expr>,
    pub(crate) runlist_stocks: Vec<Expr>,
    pub(crate) offsets: HashMap<Ident, HashMap<Ident, (usize, usize)>>,
    pub(crate) runlist_order: Vec<Ident>,
    tables: HashMap<Ident, Table>,
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
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
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
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
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
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
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
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
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
        } else if let Some(Ast::ApplyToAll(dims, _)) = model.variables[*ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
        } else if let Some(Ast::Arrayed(dims, _)) = model.variables[*ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
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
                offsets.insert(
                    format!("{}.{}", quoteize(ident), quoteize(sub_name)),
                    (i + sub_off, sub_size),
                );
            }
            let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
            sub_size
        } else if let Some(Ast::ApplyToAll(dims, _)) = &model.variables[*ident].ast() {
            for (j, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let subscript = subscripts.join(",");
                let subscripted_ident = format!("{}[{}]", quoteize(ident), subscript);
                offsets.insert(subscripted_ident, (i + j, 1));
            }
            dims.iter().map(|dim| dim.len()).product()
        } else if let Some(Ast::Arrayed(dims, _)) = &model.variables[*ident].ast() {
            for (j, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let subscript = subscripts.join(",");
                let subscripted_ident = format!("{}[{}]", quoteize(ident), subscript);
                offsets.insert(subscripted_ident, (i + j, 1));
            }
            dims.iter().map(|dim| dim.len()).product()
        } else {
            offsets.insert(quoteize(ident), (i, 1));
            1
        };
        i += size;
    }

    offsets
}

fn calc_flattened_order(sim: &Simulation, model_name: &str) -> Vec<Ident> {
    let is_root = model_name == "main";

    let module = &sim.modules[model_name];

    let mut offsets: Vec<Ident> = Vec::with_capacity(module.runlist_order.len() + 1);

    if is_root {
        offsets.push("time".to_owned());
    }

    for ident in module.runlist_order.iter() {
        // FIXME: this isnt' quite right (assumes no regular var has same name as module)
        if sim.modules.contains_key(ident) {
            let sub_var_names = calc_flattened_order(sim, ident);
            for sub_name in sub_var_names.iter() {
                offsets.push(format!("{}.{}", quoteize(ident), quoteize(sub_name)));
            }
        } else {
            offsets.push(quoteize(ident));
        }
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
    fn new(
        project: &Project,
        model: Rc<ModelStage1>,
        inputs: &BTreeSet<Ident>,
        is_root: bool,
    ) -> Result<Self> {
        let inputs_set = inputs.iter().cloned().collect::<BTreeSet<_>>();

        let instantiation = model
            .instantiations
            .as_ref()
            .and_then(|instantiations| instantiations.get(&inputs_set))
            .ok_or(Error {
                kind: ErrorKind::Simulation,
                code: ErrorCode::NotSimulatable,
                details: Some(model.name.clone()),
            })?;

        // TODO: eventually we should try to simulate subsets of the model in the face of errors
        if model.errors.is_some() && !model.errors.as_ref().unwrap().is_empty() {
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

        let build_var = |ident, is_initial| {
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
        };

        let runlist_initials = instantiation
            .runlist_initials
            .iter()
            .map(|ident| build_var(ident, true))
            .collect::<Result<Vec<Var>>>()?;

        let runlist_flows = instantiation
            .runlist_flows
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var>>>()?;

        let runlist_stocks = instantiation
            .runlist_stocks
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var>>>()?;

        let mut runlist_order = Vec::with_capacity(runlist_flows.len() + runlist_stocks.len());
        runlist_order.extend(runlist_flows.iter().map(|v| v.ident.clone()));
        runlist_order.extend(runlist_stocks.iter().map(|v| v.ident.clone()));

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
            inputs: inputs_set.into_iter().collect(),
            n_slots,
            runlist_initials,
            runlist_flows,
            runlist_stocks,
            offsets,
            runlist_order,
            tables,
        })
    }

    pub fn compile(&self) -> Result<CompiledModule> {
        Compiler::new(self).compile()
    }
}

struct Compiler<'module> {
    module: &'module Module,
    module_decls: Vec<ModuleDeclaration>,
    graphical_functions: Vec<Vec<(f64, f64)>>,
    curr_code: ByteCodeBuilder,
}

impl<'module> Compiler<'module> {
    fn new(module: &'module Module) -> Compiler {
        Compiler {
            module,
            module_decls: vec![],
            graphical_functions: vec![],
            curr_code: ByteCodeBuilder::default(),
        }
    }

    fn walk(&mut self, exprs: &[Expr]) -> Result<ByteCode> {
        for expr in exprs.iter() {
            self.walk_expr(expr)?;
        }
        self.push(Opcode::Ret);

        let curr = std::mem::take(&mut self.curr_code);

        Ok(curr.finish())
    }

    fn walk_expr(&mut self, expr: &Expr) -> Result<Option<()>> {
        let result = match expr {
            Expr::Const(value, _) => {
                let id = self.curr_code.intern_literal(*value);
                self.push(Opcode::LoadConstant { id });
                Some(())
            }
            Expr::Var(off, _) => {
                self.push(Opcode::LoadVar {
                    off: *off as VariableOffset,
                });
                Some(())
            }
            Expr::Subscript(off, indices, bounds, _) => {
                for (i, expr) in indices.iter().enumerate() {
                    self.walk_expr(expr).unwrap().unwrap();
                    let bounds = bounds[i] as VariableOffset;
                    self.push(Opcode::PushSubscriptIndex { bounds });
                }
                assert!(indices.len() == bounds.len());
                self.push(Opcode::LoadSubscript {
                    off: *off as VariableOffset,
                });
                Some(())
            }
            Expr::Dt(_) => {
                self.push(Opcode::LoadGlobalVar {
                    off: DT_OFF as VariableOffset,
                });
                Some(())
            }
            Expr::App(builtin, _) => {
                // lookups are special
                if let BuiltinFn::Lookup(ident, index, _loc) = builtin {
                    let table = &self.module.tables[ident];
                    self.graphical_functions.push(table.data.clone());
                    let gf = (self.graphical_functions.len() - 1) as GraphicalFunctionId;
                    self.walk_expr(index)?.unwrap();
                    self.push(Opcode::Lookup { gf });
                    return Ok(Some(()));
                };

                // so are module builtins
                if let BuiltinFn::IsModuleInput(ident, _loc) = builtin {
                    let id = if self.module.inputs.contains(ident) {
                        self.curr_code.intern_literal(1.0)
                    } else {
                        self.curr_code.intern_literal(0.0)
                    };
                    self.push(Opcode::LoadConstant { id });
                    return Ok(Some(()));
                };

                match builtin {
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => {
                        let off = match builtin {
                            BuiltinFn::Time => TIME_OFF,
                            BuiltinFn::TimeStep => DT_OFF,
                            BuiltinFn::StartTime => INITIAL_TIME_OFF,
                            BuiltinFn::FinalTime => FINAL_TIME_OFF,
                            _ => unreachable!(),
                        } as u16;
                        self.push(Opcode::LoadGlobalVar { off });
                        return Ok(Some(()));
                    }
                    BuiltinFn::Lookup(_, _, _) | BuiltinFn::IsModuleInput(_, _) => unreachable!(),
                    BuiltinFn::Inf | BuiltinFn::Pi => {
                        let lit = match builtin {
                            BuiltinFn::Inf => std::f64::INFINITY,
                            BuiltinFn::Pi => std::f64::consts::PI,
                            _ => unreachable!(),
                        };
                        let id = self.curr_code.intern_literal(lit);
                        self.push(Opcode::LoadConstant { id });
                        return Ok(Some(()));
                    }
                    BuiltinFn::Abs(a)
                    | BuiltinFn::Arccos(a)
                    | BuiltinFn::Arcsin(a)
                    | BuiltinFn::Arctan(a)
                    | BuiltinFn::Cos(a)
                    | BuiltinFn::Exp(a)
                    | BuiltinFn::Int(a)
                    | BuiltinFn::Ln(a)
                    | BuiltinFn::Log10(a)
                    | BuiltinFn::Sin(a)
                    | BuiltinFn::Sqrt(a)
                    | BuiltinFn::Tan(a) => {
                        self.walk_expr(a)?.unwrap();
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(Opcode::LoadConstant { id });
                        self.push(Opcode::LoadConstant { id });
                    }
                    BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) | BuiltinFn::Step(a, b) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(Opcode::LoadConstant { id });
                    }
                    BuiltinFn::Pulse(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        if c.is_some() {
                            self.walk_expr(c.as_ref().unwrap())?.unwrap()
                        } else {
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(Opcode::LoadConstant { id });
                        };
                    }
                    BuiltinFn::Ramp(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        if c.is_some() {
                            self.walk_expr(c.as_ref().unwrap())?.unwrap()
                        } else {
                            self.push(Opcode::LoadVar {
                                off: FINAL_TIME_OFF as u16,
                            });
                        };
                    }
                    BuiltinFn::SafeDiv(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        let c = c.as_ref().map(|c| self.walk_expr(c).unwrap().unwrap());
                        if c.is_none() {
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(Opcode::LoadConstant { id });
                        }
                    }
                    BuiltinFn::Mean(args) => {
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(Opcode::LoadConstant { id });

                        for arg in args.iter() {
                            self.walk_expr(arg)?.unwrap();
                            self.push(Opcode::Op2 { op: Op2::Add });
                        }

                        let id = self.curr_code.intern_literal(args.len() as f64);
                        self.push(Opcode::LoadConstant { id });
                        self.push(Opcode::Op2 { op: Op2::Div });
                        return Ok(Some(()));
                    }
                };
                let func = match builtin {
                    BuiltinFn::Lookup(_, _, _) => unreachable!(),
                    BuiltinFn::Abs(_) => BuiltinId::Abs,
                    BuiltinFn::Arccos(_) => BuiltinId::Arccos,
                    BuiltinFn::Arcsin(_) => BuiltinId::Arcsin,
                    BuiltinFn::Arctan(_) => BuiltinId::Arctan,
                    BuiltinFn::Cos(_) => BuiltinId::Cos,
                    BuiltinFn::Exp(_) => BuiltinId::Exp,
                    BuiltinFn::Inf => BuiltinId::Inf,
                    BuiltinFn::Int(_) => BuiltinId::Int,
                    BuiltinFn::IsModuleInput(_, _) => unreachable!(),
                    BuiltinFn::Ln(_) => BuiltinId::Ln,
                    BuiltinFn::Log10(_) => BuiltinId::Log10,
                    BuiltinFn::Max(_, _) => BuiltinId::Max,
                    BuiltinFn::Mean(_) => unreachable!(),
                    BuiltinFn::Min(_, _) => BuiltinId::Min,
                    BuiltinFn::Pi => BuiltinId::Pi,
                    BuiltinFn::Pulse(_, _, _) => BuiltinId::Pulse,
                    BuiltinFn::Ramp(_, _, _) => BuiltinId::Ramp,
                    BuiltinFn::SafeDiv(_, _, _) => BuiltinId::SafeDiv,
                    BuiltinFn::Sin(_) => BuiltinId::Sin,
                    BuiltinFn::Sqrt(_) => BuiltinId::Sqrt,
                    BuiltinFn::Step(_, _) => BuiltinId::Step,
                    BuiltinFn::Tan(_) => BuiltinId::Tan,
                    // handled above; we exit early
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => unreachable!(),
                };

                self.push(Opcode::Apply { func });
                Some(())
            }
            Expr::EvalModule(ident, model_name, args) => {
                for arg in args.iter() {
                    self.walk_expr(arg).unwrap().unwrap()
                }
                let module_offsets = &self.module.offsets[&self.module.ident];
                self.module_decls.push(ModuleDeclaration {
                    model_name: model_name.clone(),
                    off: module_offsets[ident].0,
                });
                let id = (self.module_decls.len() - 1) as ModuleId;

                self.push(Opcode::EvalModule {
                    id,
                    n_inputs: args.len() as u8,
                });
                None
            }
            Expr::ModuleInput(off, _) => {
                self.push(Opcode::LoadModuleInput {
                    input: *off as ModuleInputOffset,
                });
                Some(())
            }
            Expr::Op2(op, lhs, rhs, _) => {
                self.walk_expr(lhs)?.unwrap();
                self.walk_expr(rhs)?.unwrap();
                let opcode = match op {
                    BinaryOp::Add => Opcode::Op2 { op: Op2::Add },
                    BinaryOp::Sub => Opcode::Op2 { op: Op2::Sub },
                    BinaryOp::Exp => Opcode::Op2 { op: Op2::Exp },
                    BinaryOp::Mul => Opcode::Op2 { op: Op2::Mul },
                    BinaryOp::Div => Opcode::Op2 { op: Op2::Div },
                    BinaryOp::Mod => Opcode::Op2 { op: Op2::Mod },
                    BinaryOp::Gt => Opcode::Op2 { op: Op2::Gt },
                    BinaryOp::Gte => Opcode::Op2 { op: Op2::Gte },
                    BinaryOp::Lt => Opcode::Op2 { op: Op2::Lt },
                    BinaryOp::Lte => Opcode::Op2 { op: Op2::Lte },
                    BinaryOp::Eq => Opcode::Op2 { op: Op2::Eq },
                    BinaryOp::Neq => {
                        self.push(Opcode::Op2 { op: Op2::Eq });
                        Opcode::Not {}
                    }
                    BinaryOp::And => Opcode::Op2 { op: Op2::And },
                    BinaryOp::Or => Opcode::Op2 { op: Op2::Or },
                };
                self.push(opcode);
                Some(())
            }
            Expr::Op1(op, rhs, _) => {
                self.walk_expr(rhs)?.unwrap();
                match op {
                    UnaryOp::Not => self.push(Opcode::Not {}),
                };
                Some(())
            }
            Expr::If(cond, t, f, _) => {
                self.walk_expr(t)?.unwrap();
                self.walk_expr(f)?.unwrap();
                self.walk_expr(cond)?.unwrap();
                self.push(Opcode::SetCond {});
                self.push(Opcode::If {});
                Some(())
            }
            Expr::AssignCurr(off, rhs) => {
                self.walk_expr(rhs)?.unwrap();
                self.push(Opcode::AssignCurr {
                    off: *off as VariableOffset,
                });
                None
            }
            Expr::AssignNext(off, rhs) => {
                self.walk_expr(rhs)?.unwrap();
                self.push(Opcode::AssignNext {
                    off: *off as VariableOffset,
                });
                None
            }
        };
        Ok(result)
    }

    fn push(&mut self, op: Opcode) {
        self.curr_code.push_opcode(op)
    }

    fn compile(mut self) -> Result<CompiledModule> {
        let compiled_initials = Rc::new(self.walk(&self.module.runlist_initials)?);
        let compiled_flows = Rc::new(self.walk(&self.module.runlist_flows)?);
        let compiled_stocks = Rc::new(self.walk(&self.module.runlist_stocks)?);

        Ok(CompiledModule {
            ident: self.module.ident.clone(),
            n_slots: self.module.n_slots,
            context: Rc::new(ByteCodeContext {
                graphical_functions: self.graphical_functions,
                modules: self.module_decls,
            }),
            compiled_initials,
            compiled_flows,
            compiled_stocks,
        })
    }
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
            Expr::Const(n, _) => *n,
            Expr::Dt(_) => self.curr[DT_OFF],
            Expr::ModuleInput(off, _) => self.inputs[*off],
            Expr::EvalModule(ident, model_name, args) => {
                let args: Vec<f64> = args.iter().map(|arg| self.eval(arg)).collect();
                let module_offsets = &self.module.offsets[&self.module.ident];
                let off = self.off + module_offsets[ident].0;
                let module = &self.sim.modules[model_name.as_str()];

                self.sim
                    .calc(self.step_part, module, off, &args, self.curr, self.next);

                0.0
            }
            Expr::Var(off, _) => self.curr[self.off + *off],
            Expr::Subscript(off, r, bounds, _) => {
                let indices: Vec<_> = r.iter().map(|r| self.eval(r)).collect();
                let mut index = 0;
                let max_bounds = bounds.iter().product();
                let mut ok = true;
                assert_eq!(indices.len(), bounds.len());
                for (i, rhs) in indices.into_iter().enumerate() {
                    let bounds = bounds[i];
                    let one_index = rhs.floor() as usize;
                    if one_index == 0 || one_index > bounds {
                        ok = false;
                        break;
                    } else {
                        index *= bounds;
                        index += one_index - 1;
                    }
                }
                if !ok || index > max_bounds {
                    // 3.7.1 Arrays: If a subscript expression results in an invalid subscript index (i.e., it is out of range), a zero (0) MUST be returned[10]
                    // note 10: Note this can be NaN if so specified in the <uses_arrays> tag of the header options block
                    // 0 makes less sense than NaN, so lets do that until real models force us to do otherwise
                    f64::NAN
                } else {
                    self.curr[self.off + *off + index]
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
            Expr::If(cond, t, f, _) => {
                let cond: f64 = self.eval(cond);
                if is_truthy(cond) {
                    self.eval(t)
                } else {
                    self.eval(f)
                }
            }
            Expr::Op1(op, l, _) => {
                let l = self.eval(l);
                match op {
                    UnaryOp::Not => (!is_truthy(l)) as i8 as f64,
                }
            }
            Expr::Op2(op, l, r, _) => {
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
            Expr::App(builtin, _) => {
                match builtin {
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => {
                        let off = match builtin {
                            BuiltinFn::Time => TIME_OFF,
                            BuiltinFn::TimeStep => DT_OFF,
                            BuiltinFn::StartTime => INITIAL_TIME_OFF,
                            BuiltinFn::FinalTime => FINAL_TIME_OFF,
                            _ => unreachable!(),
                        };
                        self.curr[off]
                    }
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
                    BuiltinFn::IsModuleInput(ident, _) => {
                        self.module.inputs.contains(ident) as i8 as f64
                    }
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
                    BuiltinFn::Mean(args) => {
                        let count = args.len() as f64;
                        let sum: f64 = args.iter().map(|arg| self.eval(arg)).sum();
                        sum / count
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
                    BuiltinFn::Lookup(id, index, _) => {
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
                        let dt = self.curr[DT_OFF];
                        let volume = self.eval(a);
                        let first_pulse = self.eval(b);
                        let interval = match c.as_ref() {
                            Some(c) => self.eval(c),
                            None => 0.0,
                        };

                        pulse(time, dt, volume, first_pulse, interval)
                    }
                    BuiltinFn::Ramp(a, b, c) => {
                        let time = self.curr[TIME_OFF];
                        let slope = self.eval(a);
                        let start_time = self.eval(b);
                        let end_time = c.as_ref().map(|c| self.eval(c));

                        ramp(time, slope, start_time, end_time)
                    }
                    BuiltinFn::Step(a, b) => {
                        let time = self.curr[TIME_OFF];
                        let dt = self.curr[DT_OFF];
                        let height = self.eval(a);
                        let step_time = self.eval(b);

                        step(time, dt, height, step_time)
                    }
                }
            }
        }
    }
}

fn child_needs_parens(parent: &Expr, child: &Expr) -> bool {
    match parent {
        // no children so doesn't matter
        Expr::Const(_, _) | Expr::Var(_, _) => false,
        // children are comma separated, so no ambiguity possible
        Expr::App(_, _) | Expr::Subscript(_, _, _, _) => false,
        // these don't need it
        Expr::Dt(_)
        | Expr::EvalModule(_, _, _)
        | Expr::ModuleInput(_, _)
        | Expr::AssignCurr(_, _)
        | Expr::AssignNext(_, _) => false,
        Expr::Op1(_, _, _) => matches!(child, Expr::Op2(_, _, _, _)),
        Expr::Op2(parent_op, _, _, _) => match child {
            Expr::Const(_, _)
            | Expr::Var(_, _)
            | Expr::App(_, _)
            | Expr::Subscript(_, _, _, _)
            | Expr::If(_, _, _, _)
            | Expr::Dt(_)
            | Expr::EvalModule(_, _, _)
            | Expr::ModuleInput(_, _)
            | Expr::AssignCurr(_, _)
            | Expr::AssignNext(_, _)
            | Expr::Op1(_, _, _) => false,
            // 3 * 2 + 1
            Expr::Op2(child_op, _, _, _) => {
                // if we have `3 * (2 + 3)`, the parent's precedence
                // is higher than the child and we need enclosing parens
                parent_op.precedence() > child_op.precedence()
            }
        },
        Expr::If(_, _, _, _) => false,
    }
}

fn paren_if_necessary(parent: &Expr, child: &Expr, eqn: String) -> String {
    if child_needs_parens(parent, child) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

#[allow(dead_code)]
pub fn pretty(expr: &Expr) -> String {
    match expr {
        Expr::Const(n, _) => format!("{}", n),
        Expr::Var(off, _) => format!("curr[{}]", off),
        Expr::Subscript(off, args, bounds, _) => {
            let args: Vec<_> = args.iter().map(pretty).collect();
            let string_args = args.join(", ");
            let bounds: Vec<_> = bounds.iter().map(|bounds| format!("{}", bounds)).collect();
            let string_bounds = bounds.join(", ");
            format!(
                "curr[{} + (({}) - 1); bounds: {}]",
                off, string_args, string_bounds
            )
        }
        Expr::Dt(_) => "dt".to_string(),
        Expr::App(builtin, _) => match builtin {
            BuiltinFn::Time => "time".to_string(),
            BuiltinFn::TimeStep => "time_step".to_string(),
            BuiltinFn::StartTime => "initial_time".to_string(),
            BuiltinFn::FinalTime => "final_time".to_string(),
            BuiltinFn::Lookup(table, idx, _loc) => format!("lookup({}, {})", table, pretty(idx)),
            BuiltinFn::Abs(l) => format!("abs({})", pretty(l)),
            BuiltinFn::Arccos(l) => format!("arccos({})", pretty(l)),
            BuiltinFn::Arcsin(l) => format!("arcsin({})", pretty(l)),
            BuiltinFn::Arctan(l) => format!("arctan({})", pretty(l)),
            BuiltinFn::Cos(l) => format!("cos({})", pretty(l)),
            BuiltinFn::Exp(l) => format!("exp({})", pretty(l)),
            BuiltinFn::Inf => "∞".to_string(),
            BuiltinFn::Int(l) => format!("int({})", pretty(l)),
            BuiltinFn::IsModuleInput(ident, _loc) => format!("isModuleInput({})", ident),
            BuiltinFn::Ln(l) => format!("ln({})", pretty(l)),
            BuiltinFn::Log10(l) => format!("log10({})", pretty(l)),
            BuiltinFn::Max(l, r) => format!("max({}, {})", pretty(l), pretty(r)),
            BuiltinFn::Mean(args) => {
                let args: Vec<_> = args.iter().map(pretty).collect();
                let string_args = args.join(", ");
                format!("mean({})", string_args)
            }
            BuiltinFn::Min(l, r) => format!("min({}, {})", pretty(l), pretty(r)),
            BuiltinFn::Pi => "𝜋".to_string(),
            BuiltinFn::Pulse(a, b, c) => {
                let c = match c.as_ref() {
                    Some(c) => pretty(c),
                    None => "0<default>".to_owned(),
                };
                format!("pulse({}, {}, {})", pretty(a), pretty(b), c)
            }
            BuiltinFn::Ramp(a, b, c) => {
                let c = match c.as_ref() {
                    Some(c) => pretty(c),
                    None => "0<default>".to_owned(),
                };
                format!("ramp({}, {}, {})", pretty(a), pretty(b), c)
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
            BuiltinFn::Step(a, b) => {
                format!("step({}, {})", pretty(a), pretty(b))
            }
            BuiltinFn::Tan(l) => format!("tan({})", pretty(l)),
        },
        Expr::EvalModule(module, model_name, args) => {
            let args: Vec<_> = args.iter().map(pretty).collect();
            let string_args = args.join(", ");
            format!("eval<{}::{}>({})", module, model_name, string_args)
        }
        Expr::ModuleInput(a, _) => format!("mi<{}>", a),
        Expr::Op2(op, l, r, _) => {
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

            format!(
                "{} {} {}",
                paren_if_necessary(expr, l, pretty(l)),
                op,
                paren_if_necessary(expr, r, pretty(r))
            )
        }
        Expr::Op1(op, l, _) => {
            let op: &str = match op {
                UnaryOp::Not => "!",
            };
            format!("{}{}", op, paren_if_necessary(expr, l, pretty(l)))
        }
        Expr::If(cond, l, r, _) => {
            format!("if {} then {} else {}", pretty(cond), pretty(l), pretty(r))
        }
        Expr::AssignCurr(off, rhs) => format!("curr[{}] := {}", off, pretty(rhs)),
        Expr::AssignNext(off, rhs) => format!("next[{}] := {}", off, pretty(rhs)),
    }
}

#[derive(Debug)]
pub struct Simulation {
    pub(crate) modules: HashMap<Ident, Module>,
    specs: Specs,
    root: String,
    offsets: HashMap<Ident, usize>,
}

impl Simulation {
    pub fn new(project: &Project, main_model_name: &str) -> Result<Self> {
        if !project.models.contains_key(main_model_name) {
            return sim_err!(
                NotSimulatable,
                format!("no model named '{}' to simulate", main_model_name)
            );
        }

        let modules = {
            let project_models: HashMap<_, _> = project
                .models
                .iter()
                .map(|(name, model)| (name.as_str(), model.as_ref()))
                .collect();
            // then pull in all the module instantiations the main model depends on
            enumerate_modules(&project_models, main_model_name, |model| model.name.clone())?
        };

        let module_names: Vec<&str> = {
            let mut module_names: Vec<&str> = modules.keys().map(|id| id.as_str()).collect();
            module_names.sort_unstable();

            let mut sorted_names = vec![main_model_name];
            sorted_names.extend(module_names.into_iter().filter(|n| *n != main_model_name));
            sorted_names
        };

        let mut compiled_modules: HashMap<Ident, Module> = HashMap::new();
        for name in module_names {
            let distinct_inputs = &modules[name];
            for inputs in distinct_inputs.iter() {
                let model = Rc::clone(&project.models[name]);
                let is_root = name == main_model_name;
                let module = Module::new(project, model, inputs, is_root)?;
                compiled_modules.insert(name.to_string(), module);
            }
        }

        let specs = Specs::from(&project.datamodel.sim_specs);

        let offsets = calc_flattened_offsets(project, main_model_name);
        let offsets: HashMap<Ident, usize> =
            offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

        Ok(Simulation {
            modules: compiled_modules,
            specs,
            root: main_model_name.to_string(),
            offsets,
        })
    }

    pub fn compile(&self) -> Result<CompiledSimulation> {
        let modules: Result<HashMap<String, CompiledModule>> = self
            .modules
            .iter()
            .map(|(name, module)| module.compile().map(|module| (name.clone(), module)))
            .collect();

        Ok(CompiledSimulation {
            modules: modules?,
            specs: self.specs.clone(),
            root: self.root.clone(),
            offsets: self.offsets.clone(),
        })
    }

    pub fn runlist_order(&self) -> Vec<Ident> {
        calc_flattened_order(self, "main")
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

        Ok(Results {
            offsets: self.offsets.clone(),
            data: boxed_slab,
            step_size: n_slots,
            step_count: n_chunks,
            specs: spec.clone(),
            is_vensim: false,
        })
    }
}

#[test]
fn test_arrays() {
    let project = {
        use crate::datamodel::*;
        Project {
            name: "arrays".to_owned(),
            source: None,
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 12.0,
                dt: Dt::Dt(0.25),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_owned()),
            },
            dimensions: vec![Dimension::Named(
                "letters".to_owned(),
                vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            )],
            units: vec![],
            models: vec![Model {
                name: "main".to_owned(),
                variables: vec![
                    Variable::Aux(Aux {
                        ident: "constants".to_owned(),
                        equation: Equation::Arrayed(
                            vec!["letters".to_owned()],
                            vec![
                                ("a".to_owned(), "9".to_owned(), None),
                                ("b".to_owned(), "7".to_owned(), None),
                                ("c".to_owned(), "5".to_owned(), None),
                            ],
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked".to_owned(),
                        equation: Equation::Scalar("aux[INT(TIME MOD 5) + 1]".to_owned(), None),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                    }),
                    Variable::Aux(Aux {
                        ident: "aux".to_owned(),
                        equation: Equation::ApplyToAll(
                            vec!["letters".to_owned()],
                            "constants".to_owned(),
                            None,
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked2".to_owned(),
                        equation: Equation::Scalar("aux[b]".to_owned(), None),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
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
            inputs: &BTreeSet::new(),
        },
        arrayed_constants_var,
    );

    assert!(parsed_var.is_ok());

    let expected = Var {
        ident: arrayed_constants_var.ident().to_owned(),
        ast: vec![
            Expr::AssignCurr(7, Box::new(Expr::Const(9.0, Loc::default()))),
            Expr::AssignCurr(8, Box::new(Expr::Const(7.0, Loc::default()))),
            Expr::AssignCurr(9, Box::new(Expr::Const(5.0, Loc::default()))),
        ],
    };
    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

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
            inputs: &BTreeSet::new(),
        },
        arrayed_aux_var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: arrayed_aux_var.ident().to_owned(),
        ast: vec![
            Expr::AssignCurr(4, Box::new(Expr::Var(7, Loc::default()))),
            Expr::AssignCurr(5, Box::new(Expr::Var(8, Loc::default()))),
            Expr::AssignCurr(6, Box::new(Expr::Var(9, Loc::default()))),
        ],
    };
    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

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
            inputs: &BTreeSet::new(),
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: var.ident().to_owned(),
        ast: vec![Expr::AssignCurr(
            11,
            Box::new(Expr::Subscript(
                4,
                vec![Expr::Const(2.0, Loc::default())],
                vec![3],
                Loc::default(),
            )),
        )],
    };

    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

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
            inputs: &BTreeSet::new(),
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: var.ident().to_owned(),
        ast: vec![Expr::AssignCurr(
            10,
            Box::new(Expr::Subscript(
                4,
                vec![Expr::Op2(
                    BinaryOp::Add,
                    Box::new(Expr::App(
                        BuiltinFn::Int(Box::new(Expr::Op2(
                            BinaryOp::Mod,
                            Box::new(Expr::App(BuiltinFn::Time, Loc::default())),
                            Box::new(Expr::Const(5.0, Loc::default())),
                            Loc::default(),
                        ))),
                        Loc::default(),
                    )),
                    Box::new(Expr::Const(1.0, Loc::default())),
                    Loc::default(),
                )],
                vec![3],
                Loc::default(),
            )),
        )],
    };

    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let sim = Simulation::new(&parsed_project, "main");
    assert!(sim.is_ok());
}

#[test]
fn nan_is_approx_eq() {
    assert!(approx_eq!(f64, f64::NAN, f64::NAN));
}
