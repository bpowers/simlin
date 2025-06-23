// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{EquationResult, Ident};
use crate::dimensions::{DimensionRange};

/// IndexExpr1 represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr2 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(Ident, Loc),
    Range(Expr2, Expr2, Loc),
    Expr(Expr2),
}

impl IndexExpr2 {
    pub(crate) fn from(expr: IndexExpr1) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr1::Wildcard(loc) => IndexExpr2::Wildcard(loc),
            IndexExpr1::StarRange(ident, loc) => IndexExpr2::StarRange(ident, loc),
            IndexExpr1::Range(l, r, loc) => {
                IndexExpr2::Range(Expr2::from(l)?, Expr2::from(r)?, loc)
            }
            IndexExpr1::Expr(e) => IndexExpr2::Expr(Expr2::from(e)?),
        };

        Ok(expr)
    }
}

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[allow(dead_code)]
#[derive(PartialEq, Clone, Debug)]
pub enum Expr2 {
    Const(String, f64, Loc),
    Var(Ident, Option<DimensionRange>, Loc),
    App(BuiltinFn<Expr2>, Option<DimensionRange>, Loc),
    Subscript(Ident, Vec<IndexExpr2>, Option<DimensionRange>, Loc),
    Op1(UnaryOp, Box<Expr2>, Option<DimensionRange>, Loc),
    Op2(
        BinaryOp,
        Box<Expr2>,
        Box<Expr2>,
        Option<DimensionRange>,
        Loc,
    ),
    If(
        Box<Expr2>,
        Box<Expr2>,
        Box<Expr2>,
        Option<DimensionRange>,
        Loc,
    ),
}

impl Expr2 {
    #[allow(dead_code)]
    pub(crate) fn from(expr: Expr1) -> EquationResult<Self> {
        let expr = match expr {
            Expr1::Const(s, n, loc) => Expr2::Const(s, n, loc),
            Expr1::Var(id, loc) => Expr2::Var(id, None, loc),
            Expr1::App(builtin_fn, loc) => {
                use BuiltinFn::*;
                let builtin = match builtin_fn {
                    Lookup(v, e, loc) => Lookup(v, Box::new(Expr2::from(*e)?), loc),
                    Abs(e) => Abs(Box::new(Expr2::from(*e)?)),
                    Arccos(e) => Arccos(Box::new(Expr2::from(*e)?)),
                    Arcsin(e) => Arcsin(Box::new(Expr2::from(*e)?)),
                    Arctan(e) => Arctan(Box::new(Expr2::from(*e)?)),
                    Cos(e) => Cos(Box::new(Expr2::from(*e)?)),
                    Exp(e) => Exp(Box::new(Expr2::from(*e)?)),
                    Inf => Inf,
                    Int(e) => Int(Box::new(Expr2::from(*e)?)),
                    IsModuleInput(s, loc) => IsModuleInput(s, loc),
                    Ln(e) => Ln(Box::new(Expr2::from(*e)?)),
                    Log10(e) => Log10(Box::new(Expr2::from(*e)?)),
                    Max(e1, e2) => Max(
                        Box::new(Expr2::from(*e1)?),
                        e2.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Mean(exprs) => {
                        let exprs: EquationResult<Vec<Expr2>> =
                            exprs.into_iter().map(Expr2::from).collect();
                        Mean(exprs?)
                    }
                    Min(e1, e2) => Min(
                        Box::new(Expr2::from(*e1)?),
                        e2.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Pi => Pi,
                    Pulse(e1, e2, e3) => Pulse(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Ramp(e1, e2, e3) => Ramp(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    SafeDiv(e1, e2, e3) => SafeDiv(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Sin(e) => Sin(Box::new(Expr2::from(*e)?)),
                    Sqrt(e) => Sqrt(Box::new(Expr2::from(*e)?)),
                    Step(e1, e2) => Step(Box::new(Expr2::from(*e1)?), Box::new(Expr2::from(*e2)?)),
                    Tan(e) => Tan(Box::new(Expr2::from(*e)?)),
                    Time => Time,
                    TimeStep => TimeStep,
                    StartTime => StartTime,
                    FinalTime => FinalTime,
                    Rank(e, opt) => Rank(
                        Box::new(Expr2::from(*e)?),
                        opt.map(|(e1, opt_e2)| {
                            Ok::<_, crate::common::EquationError>((
                                Box::new(Expr2::from(*e1)?),
                                opt_e2.map(|e2| Expr2::from(*e2)).transpose()?.map(Box::new),
                            ))
                        })
                        .transpose()?,
                    ),
                    Size(e) => Size(Box::new(Expr2::from(*e)?)),
                    Stddev(e) => Stddev(Box::new(Expr2::from(*e)?)),
                    Sum(e) => Sum(Box::new(Expr2::from(*e)?)),
                };
                Expr2::App(builtin, None, loc)
            }
            Expr1::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr2>> =
                    args.into_iter().map(IndexExpr2::from).collect();
                Expr2::Subscript(id, args?, None, loc)
            }
            Expr1::Op1(op, l, loc) => Expr2::Op1(op, Box::new(Expr2::from(*l)?), None, loc),
            Expr1::Op2(op, l, r, loc) => Expr2::Op2(
                op,
                Box::new(Expr2::from(*l)?),
                Box::new(Expr2::from(*r)?),
                None,
                loc,
            ),
            Expr1::If(cond, t, f, loc) => Expr2::If(
                Box::new(Expr2::from(*cond)?),
                Box::new(Expr2::from(*t)?),
                Box::new(Expr2::from(*f)?),
                None,
                loc,
            ),
        };
        Ok(expr)
    }
}
