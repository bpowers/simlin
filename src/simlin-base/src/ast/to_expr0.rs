// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A lowered expression as the parsed expression it says: the map from
//! [`Expr2`] back to [`Expr0`], which involves no text.

use crate::builtins::{BuiltinFn, UntypedBuiltinFn};
use crate::common::{Canonical, Ident, RawIdent};

use super::{Expr0, Expr2, IndexExpr0, IndexExpr2, print_eqn};

/// A lowered expression as equation text.
pub fn expr2_to_string(expr: &Expr2) -> String {
    let expr0 = expr2_to_expr0(expr);
    print_eqn(&expr0)
}

/// A canonical name as the identifier a parsed expression holds for it. The
/// canonical form is carried over as it is: it reads back as itself, where
/// the source form (`Ident::to_source_repr`) would read a literal period back
/// as a module separator and name another variable.
fn raw_ident(ident: &Ident<Canonical>) -> RawIdent {
    RawIdent::new(ident.as_str().to_string())
}

/// A lowered expression as a parsed one, its array bounds left behind.
pub fn expr2_to_expr0(expr: &Expr2) -> Expr0 {
    match expr {
        Expr2::Const(text, value, loc) => Expr0::Const(text.clone(), *value, *loc),
        Expr2::Var(ident, _, loc) => Expr0::Var(raw_ident(ident), *loc),
        Expr2::App(builtin, _, loc) => {
            let untyped = builtin_to_untyped(builtin);
            Expr0::App(untyped, *loc)
        }
        Expr2::Subscript(ident, indexes, _, loc) => Expr0::Subscript(
            raw_ident(ident),
            indexes.iter().map(index_expr2_to_index_expr0).collect(),
            *loc,
        ),
        Expr2::Op1(op, rhs, _, loc) => Expr0::Op1(*op, Box::new(expr2_to_expr0(rhs)), *loc),
        Expr2::Op2(op, lhs, rhs, _, loc) => Expr0::Op2(
            *op,
            Box::new(expr2_to_expr0(lhs)),
            Box::new(expr2_to_expr0(rhs)),
            *loc,
        ),
        Expr2::If(cond, then_branch, else_branch, _, loc) => Expr0::If(
            Box::new(expr2_to_expr0(cond)),
            Box::new(expr2_to_expr0(then_branch)),
            Box::new(expr2_to_expr0(else_branch)),
            *loc,
        ),
    }
}

fn index_expr2_to_index_expr0(index: &IndexExpr2) -> IndexExpr0 {
    match index {
        IndexExpr2::Wildcard(loc) => IndexExpr0::Wildcard(*loc),
        IndexExpr2::StarRange(dim, loc) => {
            IndexExpr0::StarRange(RawIdent::new(dim.as_str().to_string()), *loc)
        }
        IndexExpr2::Range(lhs, rhs, loc) => IndexExpr0::Range(
            Box::new(expr2_to_expr0(lhs)),
            Box::new(expr2_to_expr0(rhs)),
            *loc,
        ),
        IndexExpr2::DimPosition(pos, loc) => IndexExpr0::DimPosition(*pos, *loc),
        IndexExpr2::Expr(expr) => IndexExpr0::Expr(expr2_to_expr0(expr)),
    }
}

/// A typed call as the call by name that a parsed expression holds.
pub fn builtin_to_untyped(builtin: &BuiltinFn<Expr2>) -> UntypedBuiltinFn<Expr0> {
    let args: Box<[Expr0]> = match builtin {
        // The identifier payload prints as the bare variable reference it was
        // parsed from.
        BuiltinFn::IsModuleInput(ident, _) => {
            Box::new([Expr0::Var(RawIdent::new(ident.clone()), Default::default())])
        }
        other => other.args().into_iter().map(expr2_to_expr0).collect(),
    };
    UntypedBuiltinFn(builtin.name().to_string(), args)
}
