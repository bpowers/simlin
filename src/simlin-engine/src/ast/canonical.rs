// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one normal form of an equation: what two spellings of the same
//! equation share ([`CanonicalEqn`], built by [`Expr0::canonical`]).
//!
//! Every question of the form "do these two equations say the same thing?"
//! is asked here, so the answers cannot drift: whether a save kept an
//! equation (`save_check`), whether two parse-synthesized helpers are one
//! helper ([`Expr0::eq_ignoring_loc`]), whether a printed equation read back
//! as it was (the printer's round-trip property). A caller with a further
//! notion of sameness (an algebraic reading, a table beside the equation)
//! layers it over this form rather than restating it.
//!
//! The form is conservative: it identifies two spellings only where the
//! engine provably computes the same thing from both. `a + b` and `b + a`
//! are two equations.

use std::fmt;

use crate::ast::{BinaryOp, Expr0, IndexExpr0, Literal, Loc, print_eqn};
use crate::builtins::{BuiltinSig, UntypedBuiltinFn};
use crate::common::{EquationError, RawIdent, canonicalize};
use crate::lexer::LexerType;

/// Which spellings the canonical form identifies.
#[derive(Clone, Copy)]
pub enum Aliases<'a> {
    /// What a reader of the text would call the same equation: where it was
    /// written, the case and quoting of its names (the engine resolves a name
    /// by its canonical ident), and how a number is spelled (`6e+05` is
    /// `600000`).
    Spelling,
    /// Also the calls the engine reads as something it has another spelling
    /// for, each rewritten the way the engine rewrites it:
    ///
    /// - a builtin's alias as the builtin (`BuiltinSig::by_name`, the table
    ///   `Expr1` resolves a call through: `STARTTIME` is `INITIAL_TIME`);
    /// - `MODULO(a, b)` as `a mod b` ([`operator_for_call`], which the
    ///   module-function expansion applies);
    /// - `PI()` and `INF()` as the constants they push
    ///   ([`constant_for_call`]).
    ///
    /// A call is left as written where `is_macro` says the project defines a
    /// macro of its (canonical) name, which the engine expands instead; an
    /// alias also where it says so of the builtin's own name, which then
    /// names the macro while the alias still names the builtin; and
    /// no call inside a subscript is folded to a constant: the engine reads
    /// an index literal otherwise than a call there (`x[1e400]` does not
    /// compile, and `x[INF()]` does).
    Engine { is_macro: &'a dyn Fn(&str) -> bool },
}

/// An equation in its canonical form: no source positions, every name its
/// canonical ident, every function its canonical name, every number the one
/// spelling of its value, and (under [`Aliases::Engine`]) the engine's own
/// rewrites applied.
///
/// Two equations are the same exactly when their canonical forms are equal.
/// `Display` prints the form through [`print_eqn`], for a person to read.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CanonicalEqn(Expr0);

impl CanonicalEqn {
    /// The canonical form of equation text: `Ok(None)` for text with no
    /// tokens, the parse errors for text that does not parse.
    pub fn parse(
        text: &str,
        aliases: Aliases<'_>,
    ) -> Result<Option<CanonicalEqn>, Vec<EquationError>> {
        Ok(Expr0::new(text, LexerType::Equation)?.map(|expr| expr.canonical(aliases)))
    }

    /// The canonical tree, for a caller that reads its structure.
    pub fn expr(&self) -> &Expr0 {
        &self.0
    }
}

impl fmt::Display for CanonicalEqn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&print_eqn(&self.0))
    }
}

impl Expr0 {
    /// This equation's canonical form (see [`CanonicalEqn`]).
    pub fn canonical(&self, aliases: Aliases<'_>) -> CanonicalEqn {
        CanonicalEqn(canonical_expr(self, aliases, false))
    }
}

/// The binary operator a call is the function-call spelling of, as the engine
/// reads it, with its two operands (`MODULO(a, b)` is `a mod b`), or the
/// arguments back when the call is no operator's. `func` is the call's
/// canonical name.
///
/// The module-function expansion (`builtins_visitor`) and the canonical form
/// both ask this, so what the engine rewrites and what compares as the same
/// equation are one list.
pub(crate) fn operator_for_call<T>(func: &str, args: Vec<T>) -> Result<(BinaryOp, T, T), Vec<T>> {
    if func != "modulo" {
        return Err(args);
    }
    let [lhs, rhs] = <[T; 2]>::try_from(args)?;
    Ok((BinaryOp::Mod, lhs, rhs))
}

/// The constant a zero-argument call pushes: `PI()` and `INF()`
/// (`compiler::codegen`'s `BuiltinFn::Pi` and `BuiltinFn::Inf` arm, which
/// `the_constant_calls_are_the_constants_the_engine_pushes` holds this to).
/// `func` is the builtin's own name, its aliases resolved.
fn constant_for_call(func: &str, arg_count: usize) -> Option<f64> {
    if arg_count != 0 {
        return None;
    }
    match func {
        "pi" => Some(std::f64::consts::PI),
        "inf" => Some(f64::INFINITY),
        _ => None,
    }
}

/// A number as the one spelling of its value: the shortest decimal that
/// reads back as it, in exponent form when it is very large or very small.
/// Infinity (a literal too large for a number, or `INF()` folded into one) is
/// `Inf` and the NaN literal `NaN`.
fn number(value: f64) -> String {
    let magnitude = value.abs();
    if value.is_nan() {
        "NaN".to_string()
    } else if value.is_infinite() {
        if value > 0.0 { "Inf" } else { "-Inf" }.to_string()
    } else if value == 0.0 || (1e-5..1e16).contains(&magnitude) {
        format!("{value}")
    } else {
        format!("{value:e}")
    }
}

fn constant(value: f64) -> Expr0 {
    Expr0::Const(number(value), Literal::new(value), Loc::default())
}

fn canonical_ident(ident: &RawIdent) -> RawIdent {
    RawIdent::new(canonicalize(ident.as_str()).into_owned())
}

fn canonical_expr(expr: &Expr0, aliases: Aliases<'_>, in_subscript: bool) -> Expr0 {
    let loc = Loc::default();
    let walk = |e: &Expr0| Box::new(canonical_expr(e, aliases, in_subscript));
    match expr {
        Expr0::Const(_, literal, _) => constant(literal.value()),
        Expr0::Var(ident, _) => Expr0::Var(canonical_ident(ident), loc),
        Expr0::App(UntypedBuiltinFn(func, args), _) => {
            let mut func = canonicalize(func).into_owned();
            let mut args: Vec<Expr0> = args
                .iter()
                .map(|arg| canonical_expr(arg, aliases, in_subscript))
                .collect();
            if let Aliases::Engine { is_macro } = aliases
                && !is_macro(&func)
            {
                // An alias is its builtin only where no macro takes the
                // builtin's own name: the engine expands that name as the
                // macro, so the two spellings compute different things.
                if let Some(sig) = BuiltinSig::by_name(&func)
                    && !is_macro(sig.name)
                {
                    func = sig.name.to_string();
                }
                if !in_subscript {
                    if let Some(value) = constant_for_call(&func, args.len()) {
                        return constant(value);
                    }
                    match operator_for_call(&func, args) {
                        Ok((op, lhs, rhs)) => {
                            return Expr0::Op2(op, Box::new(lhs), Box::new(rhs), loc);
                        }
                        Err(not_an_operator) => args = not_an_operator,
                    }
                }
            }
            Expr0::App(UntypedBuiltinFn(func, args.into()), loc)
        }
        Expr0::Subscript(ident, indices, _) => {
            let index = |e: &Expr0| canonical_expr(e, aliases, true);
            let indices: Vec<IndexExpr0> = indices
                .iter()
                .map(|i| match i {
                    IndexExpr0::Wildcard(_) => IndexExpr0::Wildcard(loc),
                    IndexExpr0::StarRange(dim, _) => {
                        IndexExpr0::StarRange(canonical_ident(dim), loc)
                    }
                    IndexExpr0::Range(lo, hi, _) => {
                        IndexExpr0::Range(Box::new(index(lo)), Box::new(index(hi)), loc)
                    }
                    IndexExpr0::DimPosition(n, _) => IndexExpr0::DimPosition(*n, loc),
                    IndexExpr0::Expr(e) => IndexExpr0::Expr(index(e)),
                })
                .collect();
            Expr0::Subscript(canonical_ident(ident), indices.into(), loc)
        }
        Expr0::Op1(op, e, _) => Expr0::Op1(*op, walk(e), loc),
        Expr0::Op2(op, l, r, _) => Expr0::Op2(*op, walk(l), walk(r), loc),
        Expr0::If(c, t, f, _) => Expr0::If(walk(c), walk(t), walk(f), loc),
    }
}

#[cfg(test)]
#[path = "canonical_tests.rs"]
mod canonical_tests;
