// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::result::Result as StdResult;

use lalrpop_util::ParseError;

use super::expr2::{BinaryOp, UnaryOp};
use crate::builtins::{Loc, UntypedBuiltinFn, is_0_arity_builtin_fn};
use crate::common::{EquationError, Ident};
use crate::token::LexerType;

/// Expr0 represents a parsed equation, before any calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr0 {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(UntypedBuiltinFn<Expr0>, Loc),
    Subscript(Ident, Vec<IndexExpr0>, Loc),
    Op1(UnaryOp, Box<Expr0>, Loc),
    Op2(BinaryOp, Box<Expr0>, Box<Expr0>, Loc),
    If(Box<Expr0>, Box<Expr0>, Box<Expr0>, Loc),
}

#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr0 {
    Wildcard(Loc),
    StarRange(Ident, Loc),
    Range(Expr0, Expr0, Loc),
    Expr(Expr0),
}

impl IndexExpr0 {
    pub(crate) fn reify_0_arity_builtins(self) -> Self {
        match self {
            IndexExpr0::Wildcard(_) => self,
            IndexExpr0::StarRange(_, _) => self,
            IndexExpr0::Range(_, _, _) => self,
            IndexExpr0::Expr(expr) => IndexExpr0::Expr(expr.reify_0_arity_builtins()),
        }
    }

    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            IndexExpr0::Wildcard(_loc) => IndexExpr0::Wildcard(loc),
            IndexExpr0::StarRange(d, _loc) => IndexExpr0::StarRange(d, loc),
            IndexExpr0::Range(l, r, _loc) => IndexExpr0::Range(l.strip_loc(), r.strip_loc(), loc),
            IndexExpr0::Expr(e) => IndexExpr0::Expr(e.strip_loc()),
        }
    }
}

impl Expr0 {
    /// new returns a new Expression AST if one can be constructed, or a list of
    /// source/equation errors if one couldn't be constructed.
    pub fn new(eqn: &str, lexer_type: LexerType) -> StdResult<Option<Expr0>, Vec<EquationError>> {
        let lexer = crate::token::Lexer::new(eqn, lexer_type);
        match crate::equation::EquationParser::new().parse(eqn, lexer) {
            Ok(ast) => Ok(Some(match lexer_type {
                // in variable equations we want to treat `pi` or `time`
                // as calls to `pi()` or `time()` builtin functions.  But
                // in unit equations we might have a unit called "time", and
                // function calls don't make sense there anyway.  So only
                // reify for definitions/equations.
                LexerType::Equation => ast.reify_0_arity_builtins(),
                LexerType::Units => ast,
            })),
            Err(err) => {
                use crate::common::ErrorCode::*;
                let err = match err {
                    ParseError::InvalidToken { location: l } => EquationError {
                        start: l as u16,
                        end: (l + 1) as u16,
                        code: InvalidToken,
                    },
                    ParseError::UnrecognizedEof {
                        location: l,
                        expected: _e,
                    } => {
                        // if we get an EOF at position 0, that simply means
                        // we have an empty (or comment-only) equation.  Its not
                        // an _error_, but we also don't have an AST
                        if l == 0 {
                            return Ok(None);
                        }
                        // TODO: we can give a more precise error message here, including what
                        //   types of tokens would be ok
                        EquationError {
                            start: l as u16,
                            end: (l + 1) as u16,
                            code: UnrecognizedEof,
                        }
                    }
                    ParseError::UnrecognizedToken {
                        token: (l, _t, r), ..
                    } => EquationError {
                        start: l as u16,
                        end: r as u16,
                        code: UnrecognizedToken,
                    },
                    ParseError::ExtraToken {
                        token: (l, _t, r), ..
                    } => EquationError {
                        start: l as u16,
                        end: r as u16,
                        code: ExtraToken,
                    },
                    ParseError::User { error: e } => e,
                };

                Err(vec![err])
            }
        }
    }

    /// reify turns variable references to known 0-arity builtin functions
    /// like `pi()` into App()s of those functions.
    pub(crate) fn reify_0_arity_builtins(self) -> Self {
        match self {
            Expr0::Var(ref id, loc) => {
                if is_0_arity_builtin_fn(id) {
                    Expr0::App(UntypedBuiltinFn(id.clone(), vec![]), loc)
                } else {
                    self
                }
            }
            Expr0::Const(_, _, _) => self,
            Expr0::App(UntypedBuiltinFn(func, args), loc) => {
                let args = args
                    .into_iter()
                    .map(|arg| arg.reify_0_arity_builtins())
                    .collect::<Vec<_>>();
                Expr0::App(UntypedBuiltinFn(func, args), loc)
            }
            Expr0::Subscript(id, args, loc) => {
                let args = args
                    .into_iter()
                    .map(|arg| arg.reify_0_arity_builtins())
                    .collect::<Vec<_>>();
                Expr0::Subscript(id, args, loc)
            }
            Expr0::Op1(op, mut r, loc) => {
                *r = r.reify_0_arity_builtins();
                Expr0::Op1(op, r, loc)
            }
            Expr0::Op2(op, mut l, mut r, loc) => {
                *l = l.reify_0_arity_builtins();
                *r = r.reify_0_arity_builtins();
                Expr0::Op2(op, l, r, loc)
            }
            Expr0::If(mut cond, mut t, mut f, loc) => {
                *cond = cond.reify_0_arity_builtins();
                *t = t.reify_0_arity_builtins();
                *f = f.reify_0_arity_builtins();
                Expr0::If(cond, t, f, loc)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr0::Const(s, n, _loc) => Expr0::Const(s, n, loc),
            Expr0::Var(v, _loc) => Expr0::Var(v, loc),
            Expr0::App(UntypedBuiltinFn(builtin, args), _loc) => Expr0::App(
                UntypedBuiltinFn(
                    builtin,
                    args.into_iter().map(|arg| arg.strip_loc()).collect(),
                ),
                loc,
            ),
            Expr0::Subscript(off, subscripts, _) => {
                let subscripts = subscripts
                    .into_iter()
                    .map(|expr| expr.strip_loc())
                    .collect();
                Expr0::Subscript(off, subscripts, loc)
            }
            Expr0::Op1(op, r, _loc) => Expr0::Op1(op, Box::new(r.strip_loc()), loc),
            Expr0::Op2(op, l, r, _loc) => {
                Expr0::Op2(op, Box::new(l.strip_loc()), Box::new(r.strip_loc()), loc)
            }
            Expr0::If(cond, t, f, _loc) => Expr0::If(
                Box::new(cond.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                loc,
            ),
        }
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr0::Const(_, _, loc) => *loc,
            Expr0::Var(_, loc) => *loc,
            Expr0::App(_, loc) => *loc,
            Expr0::Subscript(_, _, loc) => *loc,
            Expr0::Op1(_, _, loc) => *loc,
            Expr0::Op2(_, _, _, loc) => *loc,
            Expr0::If(_, _, _, loc) => *loc,
        }
    }
}

impl Default for Expr0 {
    fn default() -> Self {
        Expr0::Const("0.0".to_string(), 0.0, Loc::default())
    }
}
