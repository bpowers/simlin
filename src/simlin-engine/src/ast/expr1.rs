// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::expr0::{Expr0, IndexExpr0};
use super::expr2::{BinaryOp, UnaryOp};
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{EquationResult, Ident};
use crate::eqn_err;
use crate::model::ScopeStage0;

/// IndexExpr1 represents a parsed equation index, after calls to
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

/// Expr1 represents a parsed equation, after calls to
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
            Expr0::App(crate::builtins::UntypedBuiltinFn(id, orig_args), loc) => {
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
