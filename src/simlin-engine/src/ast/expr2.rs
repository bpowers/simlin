// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{EquationResult, Ident};
use crate::dimensions::{DimensionRange, DimensionVec, DimensionsContext};

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
    pub(crate) fn from(expr: Expr1) -> EquationResult<Self> {
        let expr = match expr {
            Expr1::Const(s, n, loc) => Expr2::Const(s, n, loc),
            Expr1::Var(id, loc) => Expr2::Var(id, None, loc),
            Expr1::App(builtin_fn, loc) => {
                let builtin = match builtin_fn {
                    BuiltinFn::Lookup(v, e, loc) => {
                        BuiltinFn::Lookup(v, Box::new(Expr2::from(*e)?), loc)
                    }
                    BuiltinFn::Abs(e) => BuiltinFn::Abs(Box::new(Expr2::from(*e)?)),
                    BuiltinFn::Arccos(_) => {}
                    BuiltinFn::Arcsin(_) => {}
                    BuiltinFn::Arctan(_) => {}
                    BuiltinFn::Cos(_) => {}
                    BuiltinFn::Exp(_) => {}
                    BuiltinFn::Inf => {}
                    BuiltinFn::Int(_) => {}
                    BuiltinFn::IsModuleInput(_, _) => {}
                    BuiltinFn::Ln(_) => {}
                    BuiltinFn::Log10(_) => {}
                    BuiltinFn::Max(_, _) => {}
                    BuiltinFn::Mean(_) => {}
                    BuiltinFn::Min(_, _) => {}
                    BuiltinFn::Pi => {}
                    BuiltinFn::Pulse(_, _, _) => {}
                    BuiltinFn::Ramp(_, _, _) => {}
                    BuiltinFn::SafeDiv(_, _, _) => {}
                    BuiltinFn::Sin(_) => {}
                    BuiltinFn::Sqrt(_) => {}
                    BuiltinFn::Step(_, _) => {}
                    BuiltinFn::Tan(_) => {}
                    BuiltinFn::Time => {}
                    BuiltinFn::TimeStep => {}
                    BuiltinFn::StartTime => {}
                    BuiltinFn::FinalTime => {}
                    BuiltinFn::Rank(_, _) => {}
                    BuiltinFn::Size(_) => {}
                    BuiltinFn::Stddev(_) => {}
                    BuiltinFn::Sum(_) => {}
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
