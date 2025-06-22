// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub use crate::builtins::Loc;
use std::collections::HashMap;

use crate::builtins::{BuiltinContents, BuiltinFn, UntypedBuiltinFn, walk_builtin_expr};
use crate::common::{ElementName, EquationResult, Ident};
use crate::datamodel::Dimension;
use crate::eqn_err;
use crate::model::ScopeStage0;
mod expr0;
pub use expr0::{Expr0, IndexExpr0};

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionRange {
    dim: Dimension,
    start: u32,
    end: u32,
}

#[allow(dead_code)]
impl DimensionRange {
    pub fn new(dim: Dimension, start: u32, end: u32) -> Self {
        DimensionRange { dim, start, end }
    }

    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// DimensionInfo represents the array dimensions of an expression.
/// It uses the existing Dimension enum which already encapsulates
/// both name and size together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionVec {
    dims: Vec<DimensionRange>,
}

#[allow(dead_code)]
impl DimensionVec {
    /// Create dimension info from a vector of dimensions
    pub fn new(dims: Vec<DimensionRange>) -> Self {
        DimensionVec { dims }
    }

    /// Create dimension info for a scalar value (no dimensions)
    pub fn scalar() -> Self {
        DimensionVec { dims: vec![] }
    }

    /// Check if this represents a scalar value
    pub fn is_scalar(&self) -> bool {
        self.dims.is_empty()
    }

    /// Get the dimensions
    pub fn dimensions(&self) -> &[DimensionRange] {
        &self.dims
    }

    /// Get the number of dimensions
    pub fn ndim(&self) -> usize {
        self.dims.len()
    }

    /// Get the total number of elements
    pub fn size(&self) -> u32 {
        if self.is_scalar() {
            1
        } else {
            self.dims.iter().map(|d| d.len()).product()
        }
    }

    /// Get the shape as a vector of sizes
    pub fn shape(&self) -> Vec<u32> {
        self.dims.iter().map(|d| d.len()).collect()
    }

    /// Get dimension names
    pub fn names(&self) -> Vec<&str> {
        self.dims.iter().map(|d| d.dim.name()).collect()
    }

    /// Create new DimensionInfo with a subset of dimensions (for slicing)
    pub fn slice(&self, keep_dims: &[bool]) -> Self {
        assert_eq!(keep_dims.len(), self.dims.len());
        DimensionVec {
            dims: self
                .dims
                .iter()
                .zip(keep_dims.iter())
                .filter_map(|(dim, &keep)| if keep { Some(dim.clone()) } else { None })
                .collect(),
        }
    }

    /// Check if dimensions are compatible for element-wise operations
    pub fn is_compatible(&self, other: &Self) -> bool {
        self.dims == other.dims
    }
}

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr1 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(Ident, Loc),
    Range(Expr1, Expr1, Loc),
    Expr(Expr1),
}

impl IndexExpr1 {
    pub(crate) fn from(expr: IndexExpr0) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr0::Wildcard(loc) => IndexExpr1::Wildcard(loc),
            IndexExpr0::StarRange(ident, loc) => IndexExpr1::StarRange(ident, loc),
            IndexExpr0::Range(l, r, loc) => {
                IndexExpr1::Range(Expr1::from(l)?, Expr1::from(r)?, loc)
            }
            IndexExpr0::Expr(e) => IndexExpr1::Expr(Expr1::from(e)?),
        };

        Ok(expr)
    }

    pub(crate) fn constify_dimensions(self, scope: &ScopeStage0) -> Self {
        match self {
            IndexExpr1::Wildcard(loc) => IndexExpr1::Wildcard(loc),
            IndexExpr1::StarRange(id, loc) => IndexExpr1::StarRange(id, loc),
            IndexExpr1::Range(l, r, loc) => IndexExpr1::Range(
                l.constify_dimensions(scope),
                r.constify_dimensions(scope),
                loc,
            ),
            IndexExpr1::Expr(e) => IndexExpr1::Expr(e.constify_dimensions(scope)),
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            IndexExpr1::Wildcard(_) => None,
            IndexExpr1::StarRange(v, loc) => {
                if v == ident {
                    Some(*loc)
                } else {
                    None
                }
            }
            IndexExpr1::Range(l, r, _) => {
                if let Some(loc) = l.get_var_loc(ident) {
                    return Some(loc);
                }
                r.get_var_loc(ident)
            }
            IndexExpr1::Expr(e) => e.get_var_loc(ident),
        }
    }
}

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr1 {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(BuiltinFn<Expr1>, Loc),
    Subscript(Ident, Vec<IndexExpr1>, Loc),
    Op1(UnaryOp, Box<Expr1>, Loc),
    Op2(BinaryOp, Box<Expr1>, Box<Expr1>, Loc),
    If(Box<Expr1>, Box<Expr1>, Box<Expr1>, Loc),
}

impl Expr1 {
    pub(crate) fn from(expr: Expr0) -> EquationResult<Self> {
        let expr = match expr {
            Expr0::Const(s, n, loc) => Expr1::Const(s, n, loc),
            Expr0::Var(id, loc) => Expr1::Var(id, loc),
            Expr0::App(UntypedBuiltinFn(id, orig_args), loc) => {
                let args: EquationResult<Vec<Expr1>> =
                    orig_args.into_iter().map(Expr1::from).collect();
                let mut args = args?;

                macro_rules! check_arity {
                    ($builtin_fn:tt, 0) => {{
                        if !args.is_empty() {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        BuiltinFn::$builtin_fn
                    }};
                    ($builtin_fn:tt, 1) => {{
                        if args.len() != 1 {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a))
                    }};
                    ($builtin_fn:tt, 2) => {{
                        if args.len() != 2 {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        let b = args.remove(1);
                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a), Box::new(b))
                    }};
                    ($builtin_fn:tt, 1, 2) => {{
                        if args.len() == 1 {
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), None)
                        } else if args.len() == 2 {
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Some(Box::new(b)))
                        } else {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }
                    }};
                    ($builtin_fn:tt, 3) => {{
                        if args.len() != 3 {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        let c = args.remove(2);
                        let b = args.remove(1);
                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), Box::new(c))
                    }};
                    ($builtin_fn:tt, 1, 3) => {{
                        if args.len() == 1 {
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), None)
                        } else if args.len() == 2 {
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Some((Box::new(b), None)))
                        } else if args.len() == 3 {
                            let c = args.remove(2);
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(
                                Box::new(a),
                                Some((Box::new(b), Some(Box::new(c)))),
                            )
                        } else {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }
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
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }
                    }};
                }

                let builtin = match id.as_str() {
                    "lookup" => {
                        if let Some(Expr1::Var(ident, loc)) = args.first() {
                            BuiltinFn::Lookup(ident.clone(), Box::new(args[1].clone()), *loc)
                        } else {
                            return eqn_err!(BadTable, loc.start, loc.end);
                        }
                    }
                    "mean" => BuiltinFn::Mean(args),
                    "abs" => check_arity!(Abs, 1),
                    "arccos" => check_arity!(Arccos, 1),
                    "arcsin" => check_arity!(Arcsin, 1),
                    "arctan" => check_arity!(Arctan, 1),
                    "cos" => check_arity!(Cos, 1),
                    "exp" => check_arity!(Exp, 1),
                    "inf" => check_arity!(Inf, 0),
                    "int" => check_arity!(Int, 1),
                    "ismoduleinput" => {
                        if let Some(Expr1::Var(ident, loc)) = args.first() {
                            BuiltinFn::IsModuleInput(ident.clone(), *loc)
                        } else {
                            return eqn_err!(ExpectedIdent, loc.start, loc.end);
                        }
                    }
                    "ln" => check_arity!(Ln, 1),
                    "log10" => check_arity!(Log10, 1),
                    "max" => check_arity!(Max, 1, 2),
                    "min" => check_arity!(Min, 1, 2),
                    "pi" => check_arity!(Pi, 0),
                    "pulse" => check_arity!(Pulse, 2, 3),
                    "ramp" => check_arity!(Ramp, 2, 3),
                    "safediv" => check_arity!(SafeDiv, 2, 3),
                    "sin" => check_arity!(Sin, 1),
                    "sqrt" => check_arity!(Sqrt, 1),
                    "step" => check_arity!(Step, 2),
                    "tan" => check_arity!(Tan, 1),
                    "time" => check_arity!(Time, 0),
                    "time_step" | "dt" => check_arity!(TimeStep, 0),
                    "initial_time" => check_arity!(StartTime, 0),
                    "final_time" => check_arity!(FinalTime, 0),
                    "rank" => check_arity!(Rank, 1, 3),
                    "size" => check_arity!(Size, 1),
                    "stddev" => check_arity!(Stddev, 1),
                    "sum" => check_arity!(Sum, 1),
                    _ => {
                        // TODO: this could be a table reference, array reference,
                        //       or module instantiation according to 3.3.2 of the spec
                        return eqn_err!(UnknownBuiltin, loc.start, loc.end);
                    }
                };
                Expr1::App(builtin, loc)
            }
            Expr0::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr1>> =
                    args.into_iter().map(IndexExpr1::from).collect();
                Expr1::Subscript(id, args?, loc)
            }
            Expr0::Op1(op, l, loc) => Expr1::Op1(op, Box::new(Expr1::from(*l)?), loc),
            Expr0::Op2(op, l, r, loc) => Expr1::Op2(
                op,
                Box::new(Expr1::from(*l)?),
                Box::new(Expr1::from(*r)?),
                loc,
            ),
            Expr0::If(cond, t, f, loc) => Expr1::If(
                Box::new(Expr1::from(*cond)?),
                Box::new(Expr1::from(*t)?),
                Box::new(Expr1::from(*f)?),
                loc,
            ),
        };
        Ok(expr)
    }

    pub(crate) fn constify_dimensions(self, scope: &ScopeStage0) -> Self {
        match self {
            Expr1::Const(s, n, loc) => Expr1::Const(s, n, loc),
            Expr1::Var(id, loc) => {
                if let Some(off) = scope.dimensions.lookup(&id) {
                    Expr1::Const(id, off as f64, loc)
                } else {
                    Expr1::Var(id, loc)
                }
            }
            Expr1::App(func, loc) => {
                let func = match func {
                    BuiltinFn::Inf => BuiltinFn::Inf,
                    BuiltinFn::Pi => BuiltinFn::Pi,
                    BuiltinFn::Time => BuiltinFn::Time,
                    BuiltinFn::TimeStep => BuiltinFn::TimeStep,
                    BuiltinFn::StartTime => BuiltinFn::StartTime,
                    BuiltinFn::FinalTime => BuiltinFn::FinalTime,
                    BuiltinFn::Abs(a) => BuiltinFn::Abs(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Arccos(a) => {
                        BuiltinFn::Arccos(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Arcsin(a) => {
                        BuiltinFn::Arcsin(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Arctan(a) => {
                        BuiltinFn::Arctan(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Cos(a) => BuiltinFn::Cos(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Exp(a) => BuiltinFn::Exp(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Int(a) => BuiltinFn::Int(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Ln(a) => BuiltinFn::Ln(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Log10(a) => BuiltinFn::Log10(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Sin(a) => BuiltinFn::Sin(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Tan(a) => BuiltinFn::Tan(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Mean(args) => BuiltinFn::Mean(
                        args.into_iter()
                            .map(|arg| arg.constify_dimensions(scope))
                            .collect(),
                    ),
                    BuiltinFn::Max(a, b) => BuiltinFn::Max(
                        Box::new(a.constify_dimensions(scope)),
                        b.map(|expr| Box::new(expr.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Min(a, b) => BuiltinFn::Min(
                        Box::new(a.constify_dimensions(scope)),
                        b.map(|expr| Box::new(expr.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Step(a, b) => BuiltinFn::Step(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                    ),
                    BuiltinFn::IsModuleInput(id, loc) => BuiltinFn::IsModuleInput(id, loc),
                    BuiltinFn::Lookup(id, arg, loc) => {
                        BuiltinFn::Lookup(id, Box::new(arg.constify_dimensions(scope)), loc)
                    }
                    BuiltinFn::Pulse(a, b, c) => BuiltinFn::Pulse(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                        c.map(|arg| Box::new(arg.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Ramp(a, b, c) => BuiltinFn::Ramp(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                        c.map(|arg| Box::new(arg.constify_dimensions(scope))),
                    ),
                    BuiltinFn::SafeDiv(a, b, c) => BuiltinFn::SafeDiv(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                        c.map(|arg| Box::new(arg.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Rank(a, rest) => BuiltinFn::Rank(
                        Box::new(a.constify_dimensions(scope)),
                        rest.map(|(b, c)| {
                            (
                                Box::new(b.constify_dimensions(scope)),
                                c.map(|c| Box::new(c.constify_dimensions(scope))),
                            )
                        }),
                    ),
                    BuiltinFn::Size(a) => BuiltinFn::Size(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Stddev(a) => {
                        BuiltinFn::Stddev(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Sum(a) => BuiltinFn::Sum(Box::new(a.constify_dimensions(scope))),
                };
                Expr1::App(func, loc)
            }
            Expr1::Subscript(id, args, loc) => Expr1::Subscript(
                id,
                args.into_iter()
                    .map(|arg| arg.constify_dimensions(scope))
                    .collect(),
                loc,
            ),
            Expr1::Op1(op, l, loc) => Expr1::Op1(op, Box::new(l.constify_dimensions(scope)), loc),
            Expr1::Op2(op, l, r, loc) => Expr1::Op2(
                op,
                Box::new(l.constify_dimensions(scope)),
                Box::new(r.constify_dimensions(scope)),
                loc,
            ),
            Expr1::If(cond, l, r, loc) => Expr1::If(
                Box::new(cond.constify_dimensions(scope)),
                Box::new(l.constify_dimensions(scope)),
                Box::new(r.constify_dimensions(scope)),
                loc,
            ),
        }
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr1::Const(_, _, loc) => *loc,
            Expr1::Var(_, loc) => *loc,
            Expr1::App(_, loc) => *loc,
            Expr1::Subscript(_, _, loc) => *loc,
            Expr1::Op1(_, _, loc) => *loc,
            Expr1::Op2(_, _, _, loc) => *loc,
            Expr1::If(_, _, _, loc) => *loc,
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr1::Const(_s, _n, _loc) => None,
            Expr1::Var(v, loc) if v == ident => Some(*loc),
            Expr1::Var(_v, _loc) => None,
            Expr1::App(builtin, _loc) => {
                let mut loc: Option<Loc> = None;
                walk_builtin_expr(builtin, |contents| match contents {
                    BuiltinContents::Ident(id, id_loc) => {
                        if ident == id {
                            loc = Some(id_loc);
                        }
                    }
                    BuiltinContents::Expr(expr) => {
                        if loc.is_none() {
                            loc = expr.get_var_loc(ident);
                        }
                    }
                });
                loc
            }
            Expr1::Subscript(id, subscripts, loc) => {
                if id == ident {
                    let start = loc.start as usize;
                    return Some(Loc::new(start, start + id.len()));
                }
                for arg in subscripts.iter() {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr1::Op1(_op, r, _loc) => r.get_var_loc(ident),
            Expr1::Op2(_op, l, r, _loc) => l.get_var_loc(ident).or_else(|| r.get_var_loc(ident)),
            Expr1::If(cond, t, f, _loc) => cond
                .get_var_loc(ident)
                .or_else(|| t.get_var_loc(ident))
                .or_else(|| f.get_var_loc(ident)),
        }
    }
}

impl Default for Expr1 {
    fn default() -> Self {
        Expr1::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ast<Expr> {
    Scalar(Expr),
    ApplyToAll(Vec<Dimension>, Expr),
    Arrayed(Vec<Dimension>, HashMap<ElementName, Expr>),
}

impl Ast<Expr1> {
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Ast::Scalar(expr) => expr.get_var_loc(ident),
            Ast::ApplyToAll(_, expr) => expr.get_var_loc(ident),
            Ast::Arrayed(_, subscripts) => {
                for (_, expr) in subscripts.iter() {
                    if let Some(loc) = expr.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
        }
    }

    pub fn to_latex(&self) -> String {
        match self {
            Ast::Scalar(expr) => latex_eqn(expr),
            Ast::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            Ast::Arrayed(_, _) => "TODO(array)".to_owned(),
        }
    }
}

pub(crate) fn lower_ast(scope: &ScopeStage0, ast: Ast<Expr0>) -> EquationResult<Ast<Expr1>> {
    match ast {
        Ast::Scalar(expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(Ast::Scalar),
        Ast::ApplyToAll(dims, expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(|expr| Ast::ApplyToAll(dims, expr)),
        Ast::Arrayed(dims, elements) => {
            let elements: EquationResult<HashMap<ElementName, Expr1>> = elements
                .into_iter()
                .map(|(id, expr)| {
                    match Expr1::from(expr).map(|expr| expr.constify_dimensions(scope)) {
                        Ok(expr) => Ok((id, expr)),
                        Err(err) => Err(err),
                    }
                })
                .collect();
            match elements {
                Ok(elements) => Ok(Ast::Arrayed(dims, elements)),
                Err(err) => Err(err),
            }
        }
    }
}

/// Visitors walk Expr ASTs.
pub trait Visitor<T> {
    fn walk_index(&mut self, e: &IndexExpr0) -> T;
    fn walk(&mut self, e: &Expr0) -> T;
}

/// BinaryOp enumerates the different operators supported in
/// system dynamics equations.
#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Exp,
    Mul,
    Div,
    Mod,
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

impl BinaryOp {
    /// higher the precedence, the tighter the binding.
    /// e.g. Mul.precedence() > Add.precedence()
    pub(crate) fn precedence(&self) -> u8 {
        // matches equation.lalrpop
        match self {
            BinaryOp::Add => 4,
            BinaryOp::Sub => 4,
            BinaryOp::Exp => 6,
            BinaryOp::Mul => 5,
            BinaryOp::Div => 5,
            BinaryOp::Mod => 5,
            BinaryOp::Gt => 3,
            BinaryOp::Lt => 3,
            BinaryOp::Gte => 3,
            BinaryOp::Lte => 3,
            BinaryOp::Eq => 2,
            BinaryOp::Neq => 2,
            BinaryOp::And => 1,
            BinaryOp::Or => 1,
        }
    }
}

macro_rules! child_needs_parens(
    ($expr:tt, $parent:expr, $child:expr, $eqn:expr) => {{
        match $parent {
            // no children so doesn't matter
            $expr::Const(_, _, _) | $expr::Var(_, _) => false,
            // children are comma separated, so no ambiguity possible
            $expr::App(_, _) | $expr::Subscript(_, _, _) => false,
            $expr::Op1(_, _, _) => matches!($child, $expr::Op2(_, _, _, _)),
            $expr::Op2(parent_op, _, _, _) => match $child {
                $expr::Const(_, _, _)
                | $expr::Var(_, _)
                | $expr::App(_, _)
                | $expr::Subscript(_, _, _)
                | $expr::If(_, _, _, _)
                | $expr::Op1(_, _, _) => false,
                // 3 * 2 + 1
                $expr::Op2(child_op, _, _, _) => {
                    // if we have `3 * (2 + 3)`, the parent's precedence
                    // is higher than the child and we need enclosing parens
                    parent_op.precedence() > child_op.precedence()
                }
            },
            $expr::If(_, _, _, _) => false,
        }
    }}
);

fn paren_if_necessary(parent: &Expr0, child: &Expr0, eqn: String) -> String {
    if child_needs_parens!(Expr0, parent, child, eqn) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

fn paren_if_necessary1(parent: &Expr1, child: &Expr1, eqn: String) -> String {
    if child_needs_parens!(Expr1, parent, child, eqn) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
}

struct PrintVisitor {}

impl Visitor<String> for PrintVisitor {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            IndexExpr0::StarRange(id, _) => format!("*:{}", id),
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        match expr {
            Expr0::Const(s, _, _) => s.clone(),
            Expr0::Var(id, _) => id.clone(),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr0::Op1(op, l, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "!",
                };
                format!("{}{}", op, l)
            }
            Expr0::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "mod",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{} {} {}", l, op, r)
            }
            Expr0::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("if ({}) then ({}) else ({})", cond, t, f)
            }
        }
    }
}

pub fn print_eqn(expr: &Expr0) -> String {
    let mut visitor = PrintVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_print_eqn() {
    assert_eq!(
        "a + b",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a + b * c",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Mul,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a * (b + c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Add,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Negative,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "!a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Not,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Positive,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        print_eqn(&Expr0::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "lookup(a, 1.0)",
        print_eqn(&Expr0::App(
            UntypedBuiltinFn(
                "lookup".to_string(),
                vec![
                    Expr0::Var("a".to_string(), Loc::new(7, 8)),
                    Expr0::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
                ]
            ),
            Loc::new(0, 14),
        ))
    );
}

struct LatexVisitor {}

impl LatexVisitor {
    fn walk_index(&mut self, expr: &IndexExpr1) -> String {
        match expr {
            IndexExpr1::Wildcard(_) => "*".to_string(),
            IndexExpr1::StarRange(id, _) => format!("*:{}", id),
            IndexExpr1::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr1::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr1) -> String {
        match expr {
            Expr1::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{{NaN}}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr1::Var(id, _) => {
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{}}}", id)
            }
            Expr1::App(builtin, _) => {
                let mut args: Vec<String> = vec![];
                walk_builtin_expr(builtin, |contents| {
                    let arg = match contents {
                        BuiltinContents::Ident(id, _loc) => format!("\\mathrm{{{}}}", id),
                        BuiltinContents::Expr(expr) => self.walk(expr),
                    };
                    args.push(arg);
                });
                let func = builtin.name();
                format!("\\operatorname{{{}}}({})", func, args.join(", "))
            }
            Expr1::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr1::Op1(op, l, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "\\neg ",
                };
                format!("{}{}", op, l)
            }
            Expr1::Op2(op, l, r, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                let r = paren_if_necessary1(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => {
                        return format!("{}^{{{}}}", l, r);
                    }
                    BinaryOp::Mul => "\\cdot",
                    BinaryOp::Div => {
                        return format!("\\frac{{{}}}{{{}}}", l, r);
                    }
                    BinaryOp::Mod => "%",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{} {} {}", l, op, r)
            }
            Expr1::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);

                format!(
                    "\\begin{{cases}}
                     {} & \\text{{if }} {} \\\\
                     {} & \\text{{else}}
                 \\end{{cases}}",
                    t, cond, f
                )
            }
        }
    }
}

pub fn latex_eqn(expr: &Expr1) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_latex_eqn() {
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Mul,
            Box::new(Expr1::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "(\\mathrm{a\\_c} - 1) \\cdot \\mathrm{b}",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Mul,
            Box::new(Expr1::Op2(
                BinaryOp::Sub,
                Box::new(Expr1::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr1::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Mul,
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Box::new(Expr1::Op2(
                BinaryOp::Sub,
                Box::new(Expr1::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr1::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-\\mathrm{a}",
        latex_eqn(&Expr1::Op1(
            UnaryOp::Negative,
            Box::new(Expr1::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr1::Op1(
            UnaryOp::Not,
            Box::new(Expr1::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr1::Op1(
            UnaryOp::Positive,
            Box::new(Expr1::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        latex_eqn(&Expr1::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "\\operatorname{lookup}(\\mathrm{a}, 1.0)",
        latex_eqn(&Expr1::App(
            BuiltinFn::Lookup(
                "a".to_string(),
                Box::new(Expr1::Const("1.0".to_owned(), 1.0, Default::default())),
                Default::default(),
            ),
            Loc::new(0, 14),
        ))
    );
}
