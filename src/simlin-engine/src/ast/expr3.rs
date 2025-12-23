// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr2::{ArrayBounds, Expr2, IndexExpr2};
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{Canonical, CanonicalDimensionName, Ident};

#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr3 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(CanonicalDimensionName, Loc),
    Range(Expr3, Expr3, Loc),
    DimPosition(u32, Loc),
    Expr(Expr3),
}

impl IndexExpr3 {
    pub(crate) fn from_expr2(expr: &IndexExpr2) -> Self {
        match expr {
            IndexExpr2::Wildcard(loc) => IndexExpr3::Wildcard(*loc),
            IndexExpr2::StarRange(dim, loc) => IndexExpr3::StarRange(dim.clone(), *loc),
            IndexExpr2::Range(lhs, rhs, loc) => {
                IndexExpr3::Range(Expr3::from_expr2(lhs), Expr3::from_expr2(rhs), *loc)
            }
            IndexExpr2::DimPosition(pos, loc) => IndexExpr3::DimPosition(*pos, *loc),
            IndexExpr2::Expr(expr) => IndexExpr3::Expr(Expr3::from_expr2(expr)),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            IndexExpr3::Wildcard(_) => None,
            IndexExpr3::StarRange(v, loc) => {
                if v.as_str() == ident {
                    Some(*loc)
                } else {
                    None
                }
            }
            IndexExpr3::Range(l, r, _) => l.get_var_loc(ident).or_else(|| r.get_var_loc(ident)),
            IndexExpr3::DimPosition(_, _) => None,
            IndexExpr3::Expr(e) => e.get_var_loc(ident),
        }
    }
}

#[allow(dead_code)]
#[derive(PartialEq, Clone, Debug)]
pub enum Expr3 {
    Const(String, f64, Loc),
    Var(Ident<Canonical>, Option<ArrayBounds>, Loc),
    App(BuiltinFn<Expr3>, Option<ArrayBounds>, Loc),
    Subscript(Ident<Canonical>, Vec<IndexExpr3>, Option<ArrayBounds>, Loc),
    StaticSubscript(Ident<Canonical>, Vec<IndexExpr3>, Option<ArrayBounds>, Loc),
    TempArray(u32, ArrayBounds, Loc),
    TempArrayElement(u32, ArrayBounds, usize, Loc),
    Op1(UnaryOp, Box<Expr3>, Option<ArrayBounds>, Loc),
    Op2(BinaryOp, Box<Expr3>, Box<Expr3>, Option<ArrayBounds>, Loc),
    If(Box<Expr3>, Box<Expr3>, Box<Expr3>, Option<ArrayBounds>, Loc),
    AssignTemp(u32, Box<Expr3>, ArrayBounds, Loc),
}

impl Expr3 {
    pub(crate) fn from_expr2(expr: &Expr2) -> Self {
        match expr {
            Expr2::Const(text, value, loc) => Expr3::Const(text.clone(), *value, *loc),
            Expr2::Var(ident, bounds, loc) => Expr3::Var(ident.clone(), bounds.clone(), *loc),
            Expr2::App(builtin, bounds, loc) => {
                Expr3::App(map_builtin_expr(builtin), bounds.clone(), *loc)
            }
            Expr2::Subscript(ident, args, bounds, loc) => Expr3::Subscript(
                ident.clone(),
                args.iter().map(IndexExpr3::from_expr2).collect(),
                bounds.clone(),
                *loc,
            ),
            Expr2::Op1(op, rhs, bounds, loc) => {
                Expr3::Op1(*op, Box::new(Expr3::from_expr2(rhs)), bounds.clone(), *loc)
            }
            Expr2::Op2(op, lhs, rhs, bounds, loc) => Expr3::Op2(
                *op,
                Box::new(Expr3::from_expr2(lhs)),
                Box::new(Expr3::from_expr2(rhs)),
                bounds.clone(),
                *loc,
            ),
            Expr2::If(cond, t, f, bounds, loc) => Expr3::If(
                Box::new(Expr3::from_expr2(cond)),
                Box::new(Expr3::from_expr2(t)),
                Box::new(Expr3::from_expr2(f)),
                bounds.clone(),
                *loc,
            ),
        }
    }

    pub(crate) fn get_array_bounds(&self) -> Option<&ArrayBounds> {
        match self {
            Expr3::Const(_, _, _) => None,
            Expr3::Var(_, array_bounds, _) => array_bounds.as_ref(),
            Expr3::App(_, array_bounds, _) => array_bounds.as_ref(),
            Expr3::Subscript(_, _, array_bounds, _) => array_bounds.as_ref(),
            Expr3::StaticSubscript(_, _, array_bounds, _) => array_bounds.as_ref(),
            Expr3::TempArray(_, array_bounds, _) => Some(array_bounds),
            Expr3::TempArrayElement(_, _, _, _) => None,
            Expr3::Op1(_, _, array_bounds, _) => array_bounds.as_ref(),
            Expr3::Op2(_, _, _, array_bounds, _) => array_bounds.as_ref(),
            Expr3::If(_, _, _, array_bounds, _) => array_bounds.as_ref(),
            Expr3::AssignTemp(_, _, _, _) => None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr3::Const(_, _, loc) => *loc,
            Expr3::Var(_, _, loc) => *loc,
            Expr3::App(_, _, loc) => *loc,
            Expr3::Subscript(_, _, _, loc) => *loc,
            Expr3::StaticSubscript(_, _, _, loc) => *loc,
            Expr3::TempArray(_, _, loc) => *loc,
            Expr3::TempArrayElement(_, _, _, loc) => *loc,
            Expr3::Op1(_, _, _, loc) => *loc,
            Expr3::Op2(_, _, _, _, loc) => *loc,
            Expr3::If(_, _, _, _, loc) => *loc,
            Expr3::AssignTemp(_, _, _, loc) => *loc,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr3::Const(_, _, _) => None,
            Expr3::Var(v, _, loc) if v.as_str() == ident => Some(*loc),
            Expr3::Var(_, _, _) => None,
            Expr3::App(builtin, _, _) => {
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
            Expr3::Subscript(v, _args, _, loc) if v.as_str() == ident => Some(*loc),
            Expr3::Subscript(_, args, _, _) => {
                for arg in args {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr3::StaticSubscript(v, _args, _, loc) if v.as_str() == ident => Some(*loc),
            Expr3::StaticSubscript(_, args, _, _) => {
                for arg in args {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr3::TempArray(_, _, _) => None,
            Expr3::TempArrayElement(_, _, _, _) => None,
            Expr3::Op1(_, l, _, _) => l.get_var_loc(ident),
            Expr3::Op2(_, l, r, _, _) => l.get_var_loc(ident).or_else(|| r.get_var_loc(ident)),
            Expr3::If(c, t, f, _, _) => c
                .get_var_loc(ident)
                .or_else(|| t.get_var_loc(ident))
                .or_else(|| f.get_var_loc(ident)),
            Expr3::AssignTemp(_, rhs, _, _) => rhs.get_var_loc(ident),
        }
    }
}

fn map_builtin_expr(builtin: &BuiltinFn<Expr2>) -> BuiltinFn<Expr3> {
    use BuiltinFn::*;
    match builtin {
        Lookup(name, expr, loc) => Lookup(name.clone(), Box::new(Expr3::from_expr2(expr)), *loc),
        Abs(expr) => Abs(Box::new(Expr3::from_expr2(expr))),
        Arccos(expr) => Arccos(Box::new(Expr3::from_expr2(expr))),
        Arcsin(expr) => Arcsin(Box::new(Expr3::from_expr2(expr))),
        Arctan(expr) => Arctan(Box::new(Expr3::from_expr2(expr))),
        Cos(expr) => Cos(Box::new(Expr3::from_expr2(expr))),
        Exp(expr) => Exp(Box::new(Expr3::from_expr2(expr))),
        Inf => Inf,
        Int(expr) => Int(Box::new(Expr3::from_expr2(expr))),
        IsModuleInput(name, loc) => IsModuleInput(name.clone(), *loc),
        Ln(expr) => Ln(Box::new(Expr3::from_expr2(expr))),
        Log10(expr) => Log10(Box::new(Expr3::from_expr2(expr))),
        Max(lhs, rhs) => Max(
            Box::new(Expr3::from_expr2(lhs)),
            rhs.as_ref().map(|expr| Box::new(Expr3::from_expr2(expr))),
        ),
        Mean(args) => Mean(args.iter().map(Expr3::from_expr2).collect()),
        Min(lhs, rhs) => Min(
            Box::new(Expr3::from_expr2(lhs)),
            rhs.as_ref().map(|expr| Box::new(Expr3::from_expr2(expr))),
        ),
        Pi => Pi,
        Pulse(a, b, c) => Pulse(
            Box::new(Expr3::from_expr2(a)),
            Box::new(Expr3::from_expr2(b)),
            c.as_ref().map(|expr| Box::new(Expr3::from_expr2(expr))),
        ),
        Ramp(a, b, c) => Ramp(
            Box::new(Expr3::from_expr2(a)),
            Box::new(Expr3::from_expr2(b)),
            c.as_ref().map(|expr| Box::new(Expr3::from_expr2(expr))),
        ),
        SafeDiv(a, b, c) => SafeDiv(
            Box::new(Expr3::from_expr2(a)),
            Box::new(Expr3::from_expr2(b)),
            c.as_ref().map(|expr| Box::new(Expr3::from_expr2(expr))),
        ),
        Sign(expr) => Sign(Box::new(Expr3::from_expr2(expr))),
        Sin(expr) => Sin(Box::new(Expr3::from_expr2(expr))),
        Sqrt(expr) => Sqrt(Box::new(Expr3::from_expr2(expr))),
        Step(a, b) => Step(
            Box::new(Expr3::from_expr2(a)),
            Box::new(Expr3::from_expr2(b)),
        ),
        Tan(expr) => Tan(Box::new(Expr3::from_expr2(expr))),
        Time => Time,
        TimeStep => TimeStep,
        StartTime => StartTime,
        FinalTime => FinalTime,
        Rank(expr, rest) => Rank(
            Box::new(Expr3::from_expr2(expr)),
            rest.as_ref().map(|(a, b)| {
                (
                    Box::new(Expr3::from_expr2(a)),
                    b.as_ref().map(|expr| Box::new(Expr3::from_expr2(expr))),
                )
            }),
        ),
        Size(expr) => Size(Box::new(Expr3::from_expr2(expr))),
        Stddev(expr) => Stddev(Box::new(Expr3::from_expr2(expr))),
        Sum(expr) => Sum(Box::new(Expr3::from_expr2(expr))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::canonicalize;

    #[test]
    fn test_expr3_from_expr2_preserves_bounds() {
        let bounds = ArrayBounds::Named {
            name: "value".to_string(),
            dims: vec![3],
            dim_names: Some(vec!["dim".to_string()]),
        };
        let expr2 = Expr2::Var(canonicalize("value"), Some(bounds.clone()), Loc::default());
        let expr3 = Expr3::from_expr2(&expr2);
        assert_eq!(
            expr3,
            Expr3::Var(canonicalize("value"), Some(bounds), Loc::default())
        );
    }
}
