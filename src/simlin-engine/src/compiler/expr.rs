// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::BTreeSet;

use crate::ast::{ArrayView, BinaryOp, Loc};
use crate::common::{Canonical, Ident, Result};
use crate::float::SimFloat;
use crate::sim_err;

use super::dimensions::UnaryOp;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct Table<F: SimFloat> {
    pub data: Vec<(F, F)>,
}

impl<F: SimFloat> Table<F> {
    pub(super) fn new(ident: &str, t: &crate::variable::Table) -> Result<Self> {
        if t.x.len() != t.y.len() {
            return sim_err!(BadTable, ident.to_string());
        }

        let data: Vec<(F, F)> = t
            .x
            .iter()
            .copied()
            .zip(t.y.iter().copied())
            .map(|(x, y)| (F::from_f64(x), F::from_f64(y)))
            .collect();

        Ok(Self { data })
    }
}

pub(crate) type BuiltinFn<F> = crate::builtins::BuiltinFn<Expr<F>>;

/// Represents a single subscript index in a dynamic Subscript expression.
/// This enum distinguishes between single-element access and range access,
/// enabling proper bytecode generation for dynamic ranges.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub enum SubscriptIndex<F: SimFloat> {
    /// Single element access - evaluates to a 1-based index
    Single(Expr<F>),
    /// Range access - start and end expressions (1-based, inclusive)
    /// Used for dynamic ranges like arr[start:end] where bounds are variables
    Range(Expr<F>, Expr<F>),
}

impl<F: SimFloat> SubscriptIndex<F> {
    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        match self {
            SubscriptIndex::Single(expr) => SubscriptIndex::Single(expr.strip_loc()),
            SubscriptIndex::Range(start, end) => {
                SubscriptIndex::Range(start.strip_loc(), end.strip_loc())
            }
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
#[allow(dead_code)]
pub enum Expr<F: SimFloat> {
    Const(F, Loc),
    Var(usize, Loc), // offset
    /// Dynamic subscript with possible range indices
    /// (offset, subscript indices, dimension sizes, location)
    Subscript(usize, Vec<SubscriptIndex<F>>, Vec<usize>, Loc),
    StaticSubscript(usize, ArrayView, Loc), // offset, precomputed view, location
    TempArray(u32, ArrayView, Loc),         // temp id, view into temp array, location
    TempArrayElement(u32, ArrayView, usize, Loc), // temp id, view, element index, location
    Dt(Loc),
    App(BuiltinFn<F>, Loc),
    /// EvalModule(module_ident, model_name, input_set, args)
    /// input_set is needed to look up the correct compiled module when a model has multiple instantiations
    EvalModule(
        Ident<Canonical>,
        Ident<Canonical>,
        BTreeSet<Ident<Canonical>>,
        Vec<Expr<F>>,
    ),
    ModuleInput(usize, Loc),
    Op2(BinaryOp, Box<Expr<F>>, Box<Expr<F>>, Loc),
    Op1(UnaryOp, Box<Expr<F>>, Loc),
    If(Box<Expr<F>>, Box<Expr<F>>, Box<Expr<F>>, Loc),
    AssignCurr(usize, Box<Expr<F>>),
    AssignNext(usize, Box<Expr<F>>),
    AssignTemp(u32, Box<Expr<F>>, ArrayView), // temp id, expression to evaluate, view info
}

impl<F: SimFloat> Expr<F> {
    pub(super) fn get_loc(&self) -> Loc {
        match self {
            Expr::Const(_, loc) => *loc,
            Expr::Var(_, loc) => *loc,
            Expr::Subscript(_, _, _, loc) => *loc,
            Expr::StaticSubscript(_, _, loc) => *loc,
            Expr::TempArray(_, _, loc) => *loc,
            Expr::TempArrayElement(_, _, _, loc) => *loc,
            Expr::Dt(loc) => *loc,
            Expr::App(_, loc) => *loc,
            Expr::EvalModule(_, _, _, _) => Loc::default(),
            Expr::ModuleInput(_, loc) => *loc,
            Expr::Op2(_, _, _, loc) => *loc,
            Expr::Op1(_, _, loc) => *loc,
            Expr::If(_, _, _, loc) => *loc,
            Expr::AssignCurr(_, _) => Loc::default(),
            Expr::AssignNext(_, _) => Loc::default(),
            Expr::AssignTemp(_, _, _) => Loc::default(),
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
            Expr::StaticSubscript(off, view, _) => Expr::StaticSubscript(off, view, loc),
            Expr::TempArray(id, view, _) => Expr::TempArray(id, view, loc),
            Expr::TempArrayElement(id, view, idx, _) => Expr::TempArrayElement(id, view, idx, loc),
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
                    BuiltinFn::LookupForward(id, a, _loc) => {
                        BuiltinFn::LookupForward(id, Box::new(a.strip_loc()), loc)
                    }
                    BuiltinFn::LookupBackward(id, a, _loc) => {
                        BuiltinFn::LookupBackward(id, Box::new(a.strip_loc()), loc)
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
                    BuiltinFn::Sign(a) => BuiltinFn::Sign(Box::new(a.strip_loc())),
                    BuiltinFn::Sin(a) => BuiltinFn::Sin(Box::new(a.strip_loc())),
                    BuiltinFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(a.strip_loc())),
                    BuiltinFn::Tan(a) => BuiltinFn::Tan(Box::new(a.strip_loc())),
                    BuiltinFn::Max(a, b) => {
                        BuiltinFn::Max(Box::new(a.strip_loc()), b.map(|b| Box::new(b.strip_loc())))
                    }
                    BuiltinFn::Min(a, b) => {
                        BuiltinFn::Min(Box::new(a.strip_loc()), b.map(|b| Box::new(b.strip_loc())))
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
                    BuiltinFn::Rank(a, rest) => BuiltinFn::Rank(
                        Box::new(a.strip_loc()),
                        rest.map(|(b, c)| {
                            (Box::new(b.strip_loc()), c.map(|c| Box::new(c.strip_loc())))
                        }),
                    ),
                    BuiltinFn::Size(a) => BuiltinFn::Size(Box::new(a.strip_loc())),
                    BuiltinFn::Stddev(a) => BuiltinFn::Stddev(Box::new(a.strip_loc())),
                    BuiltinFn::Sum(a) => BuiltinFn::Sum(Box::new(a.strip_loc())),
                };
                Expr::App(builtin, loc)
            }
            Expr::EvalModule(id1, id2, input_set, args) => {
                let args = args.into_iter().map(|expr| expr.strip_loc()).collect();
                Expr::EvalModule(id1, id2, input_set, args)
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
            Expr::AssignTemp(id, rhs, view) => {
                Expr::AssignTemp(id, Box::new(rhs.strip_loc()), view)
            }
        }
    }
}

#[allow(dead_code)]
pub(super) fn decompose_array_temps<F: SimFloat>(
    expr: Expr<F>,
    next_temp_id: usize,
) -> Result<(Expr<F>, Vec<Expr<F>>, usize)> {
    Ok((expr, vec![], next_temp_id))
}
