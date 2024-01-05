// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::result::Result as StdResult;

use lalrpop_util::ParseError;

pub use crate::builtins::Loc;

use crate::builtins::{
    is_0_arity_builtin_fn, walk_builtin_expr, BuiltinContents, BuiltinFn, UntypedBuiltinFn,
};
use crate::common::{ElementName, EquationError, EquationResult, Ident};
use crate::datamodel::Dimension;
use crate::eqn_err;
use crate::model::ScopeStage0;
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
    fn reify_0_arity_builtins(self) -> Self {
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
        let mut errs = Vec::new();

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

                errs.push(err);

                Err(errs)
            }
        }
    }

    /// reify turns variable references to known 0-arity builtin functions
    /// like `pi()` into App()s of those functions.
    fn reify_0_arity_builtins(self) -> Self {
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

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(Ident, Loc),
    Range(Expr, Expr, Loc),
    Expr(Expr),
}

impl IndexExpr {
    pub(crate) fn from(expr: IndexExpr0) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr0::Wildcard(loc) => IndexExpr::Wildcard(loc),
            IndexExpr0::StarRange(ident, loc) => IndexExpr::StarRange(ident, loc),
            IndexExpr0::Range(l, r, loc) => IndexExpr::Range(Expr::from(l)?, Expr::from(r)?, loc),
            IndexExpr0::Expr(e) => IndexExpr::Expr(Expr::from(e)?),
        };

        Ok(expr)
    }

    pub(crate) fn constify_dimensions(self, scope: &ScopeStage0) -> Self {
        match self {
            IndexExpr::Wildcard(loc) => IndexExpr::Wildcard(loc),
            IndexExpr::StarRange(id, loc) => IndexExpr::StarRange(id, loc),
            IndexExpr::Range(l, r, loc) => IndexExpr::Range(
                l.constify_dimensions(scope),
                r.constify_dimensions(scope),
                loc,
            ),
            IndexExpr::Expr(e) => IndexExpr::Expr(e.constify_dimensions(scope)),
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            IndexExpr::Wildcard(_) => None,
            IndexExpr::StarRange(v, loc) => {
                if v == ident {
                    Some(*loc)
                } else {
                    None
                }
            }
            IndexExpr::Range(l, r, _) => {
                if let Some(loc) = l.get_var_loc(ident) {
                    return Some(loc);
                }
                r.get_var_loc(ident)
            }
            IndexExpr::Expr(e) => e.get_var_loc(ident),
        }
    }
}

impl Default for Expr0 {
    fn default() -> Self {
        Expr0::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(BuiltinFn<Expr>, Loc),
    Subscript(Ident, Vec<IndexExpr>, Loc),
    Op1(UnaryOp, Box<Expr>, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, Loc),
}

impl Expr {
    pub(crate) fn from(expr: Expr0) -> EquationResult<Self> {
        let expr = match expr {
            Expr0::Const(s, n, loc) => Expr::Const(s, n, loc),
            Expr0::Var(id, loc) => Expr::Var(id, loc),
            Expr0::App(UntypedBuiltinFn(id, orig_args), loc) => {
                let args: EquationResult<Vec<Expr>> =
                    orig_args.into_iter().map(Expr::from).collect();
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
                    ($builtin_fn:tt, 3) => {{
                        if args.len() != 3 {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        let c = args.remove(2);
                        let b = args.remove(1);
                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), Box::new(c))
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
                        if let Some(Expr::Var(ident, loc)) = args.first() {
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
                        if let Some(Expr::Var(ident, loc)) = args.first() {
                            BuiltinFn::IsModuleInput(ident.clone(), *loc)
                        } else {
                            return eqn_err!(ExpectedIdent, loc.start, loc.end);
                        }
                    }
                    "ln" => check_arity!(Ln, 1),
                    "log10" => check_arity!(Log10, 1),
                    "max" => check_arity!(Max, 2),
                    "min" => check_arity!(Min, 2),
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
                    _ => {
                        // TODO: this could be a table reference, array reference,
                        //       or module instantiation according to 3.3.2 of the spec
                        return eqn_err!(UnknownBuiltin, loc.start, loc.end);
                    }
                };
                Expr::App(builtin, loc)
            }
            Expr0::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr>> =
                    args.into_iter().map(IndexExpr::from).collect();
                Expr::Subscript(id, args?, loc)
            }
            Expr0::Op1(op, l, loc) => Expr::Op1(op, Box::new(Expr::from(*l)?), loc),
            Expr0::Op2(op, l, r, loc) => Expr::Op2(
                op,
                Box::new(Expr::from(*l)?),
                Box::new(Expr::from(*r)?),
                loc,
            ),
            Expr0::If(cond, t, f, loc) => Expr::If(
                Box::new(Expr::from(*cond)?),
                Box::new(Expr::from(*t)?),
                Box::new(Expr::from(*f)?),
                loc,
            ),
        };
        Ok(expr)
    }

    pub(crate) fn constify_dimensions(self, scope: &ScopeStage0) -> Self {
        match self {
            Expr::Const(s, n, loc) => Expr::Const(s, n, loc),
            Expr::Var(id, loc) => {
                if let Some(off) = scope.dimensions.lookup(&id) {
                    Expr::Const(id, off as f64, loc)
                } else {
                    Expr::Var(id, loc)
                }
            }
            Expr::App(func, loc) => {
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
                        Box::new(b.constify_dimensions(scope)),
                    ),
                    BuiltinFn::Min(a, b) => BuiltinFn::Min(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
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
                };
                Expr::App(func, loc)
            }
            Expr::Subscript(id, args, loc) => Expr::Subscript(
                id,
                args.into_iter()
                    .map(|arg| arg.constify_dimensions(scope))
                    .collect(),
                loc,
            ),
            Expr::Op1(op, l, loc) => Expr::Op1(op, Box::new(l.constify_dimensions(scope)), loc),
            Expr::Op2(op, l, r, loc) => Expr::Op2(
                op,
                Box::new(l.constify_dimensions(scope)),
                Box::new(r.constify_dimensions(scope)),
                loc,
            ),
            Expr::If(cond, l, r, loc) => Expr::If(
                Box::new(cond.constify_dimensions(scope)),
                Box::new(l.constify_dimensions(scope)),
                Box::new(r.constify_dimensions(scope)),
                loc,
            ),
        }
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr::Const(_, _, loc) => *loc,
            Expr::Var(_, loc) => *loc,
            Expr::App(_, loc) => *loc,
            Expr::Subscript(_, _, loc) => *loc,
            Expr::Op1(_, _, loc) => *loc,
            Expr::Op2(_, _, _, loc) => *loc,
            Expr::If(_, _, _, loc) => *loc,
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr::Const(_s, _n, _loc) => None,
            Expr::Var(v, loc) if v == ident => Some(*loc),
            Expr::Var(_v, _loc) => None,
            Expr::App(builtin, _loc) => {
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
            Expr::Subscript(id, subscripts, loc) => {
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
            Expr::Op1(_op, r, _loc) => r.get_var_loc(ident),
            Expr::Op2(_op, l, r, _loc) => l.get_var_loc(ident).or_else(|| r.get_var_loc(ident)),
            Expr::If(cond, t, f, _loc) => cond
                .get_var_loc(ident)
                .or_else(|| t.get_var_loc(ident))
                .or_else(|| f.get_var_loc(ident)),
        }
    }
}

impl Default for Expr {
    fn default() -> Self {
        Expr::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

#[test]
fn test_parse() {
    use crate::ast::BinaryOp::*;
    use crate::ast::Expr0::*;

    let if1 = Box::new(If(
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Const("3".to_string(), 3.0, Loc::default())),
        Loc::default(),
    ));

    let if2 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("blerg".to_string(), Loc::default())),
            Box::new(Var("foo".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Const("3".to_string(), 3.0, Loc::default())),
        Loc::default(),
    ));

    let if3 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("quotient".to_string(), Loc::default())),
            Box::new(Var("quotient_target".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("0".to_string(), 0.0, Loc::default())),
        Loc::default(),
    ));

    let if4 = Box::new(If(
        Box::new(Op2(
            And,
            Box::new(Var("true_input".to_string(), Loc::default())),
            Box::new(Var("false_input".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("0".to_string(), 0.0, Loc::default())),
        Loc::default(),
    ));

    let quoting_eq = Box::new(Op2(
        Eq,
        Box::new(Var("oh_dear".to_string(), Loc::default())),
        Box::new(Var("oh_dear".to_string(), Loc::default())),
        Loc::default(),
    ));

    let subscript1 = Box::new(Subscript(
        "a".to_owned(),
        vec![IndexExpr0::Expr(Const("1".to_owned(), 1.0, Loc::default()))],
        Loc::default(),
    ));
    let subscript2 = Box::new(Subscript(
        "a".to_owned(),
        vec![
            IndexExpr0::Expr(Const("2".to_owned(), 2.0, Loc::default())),
            IndexExpr0::Expr(App(
                UntypedBuiltinFn("int".to_owned(), vec![Var("b".to_owned(), Loc::default())]),
                Loc::default(),
            )),
        ],
        Loc::default(),
    ));

    let subscript3 = Box::new(Subscript(
        "a".to_string(),
        vec![
            IndexExpr0::Wildcard(Loc::default()),
            IndexExpr0::Wildcard(Loc::default()),
        ],
        Loc::default(),
    ));

    let subscript4 = Box::new(Subscript(
        "a".to_string(),
        vec![IndexExpr0::StarRange("d".to_string(), Loc::default())],
        Loc::default(),
    ));

    let subscript5 = Box::new(Subscript(
        "a".to_string(),
        vec![IndexExpr0::Range(
            Const("1".to_owned(), 1.0, Loc::default()),
            Const("2".to_owned(), 2.0, Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    ));

    let subscript6 = Box::new(Subscript(
        "a".to_string(),
        vec![IndexExpr0::Range(
            Var("l".to_owned(), Loc::default()),
            Var("r".to_owned(), Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    ));

    let time1 = Box::new(App(
        UntypedBuiltinFn("time".to_owned(), vec![]),
        Loc::default(),
    ));

    let time2 = Box::new(Subscript(
        "aux".to_owned(),
        vec![IndexExpr0::Expr(Op2(
            BinaryOp::Add,
            Box::new(App(
                UntypedBuiltinFn(
                    "int".to_owned(),
                    vec![Op2(
                        BinaryOp::Mod,
                        Box::new(App(
                            UntypedBuiltinFn("time".to_owned(), vec![]),
                            Loc::default(),
                        )),
                        Box::new(Const("5".to_owned(), 5.0, Loc::default())),
                        Loc::default(),
                    )],
                ),
                Loc::default(),
            )),
            Box::new(Const("1".to_owned(), 1.0, Loc::default())),
            Loc::default(),
        ))],
        Loc::default(),
    ));

    let cases = [
        (
            "aux[INT(TIME MOD 5) + 1]",
            time2,
            "aux[int(time() mod 5) + 1]",
        ),
        ("if 1 then 2 else 3", if1, "if (1) then (2) else (3)"),
        (
            "if blerg = foo then 2 else 3",
            if2,
            "if (blerg = foo) then (2) else (3)",
        ),
        (
            "IF quotient = quotient_target THEN 1 ELSE 0",
            if3.clone(),
            "if (quotient = quotient_target) then (1) else (0)",
        ),
        (
            "(IF quotient = quotient_target THEN 1 ELSE 0)",
            if3,
            "if (quotient = quotient_target) then (1) else (0)",
        ),
        (
            "( IF true_input and false_input THEN 1 ELSE 0 )",
            if4.clone(),
            "if (true_input && false_input) then (1) else (0)",
        ),
        (
            "( IF true_input && false_input THEN 1 ELSE 0 )",
            if4,
            "if (true_input && false_input) then (1) else (0)",
        ),
        ("\"oh dear\" = oh_dear", quoting_eq, "oh_dear = oh_dear"),
        ("a[1]", subscript1, "a[1]"),
        ("a[2, INT(b)]", subscript2, "a[2, int(b)]"),
        ("time", time1, "time()"),
        ("a[*, *]", subscript3, "a[*, *]"),
        ("a[*:d]", subscript4, "a[*:d]"),
        ("a[1:2]", subscript5, "a[1:2]"),
        ("a[l:r]", subscript6, "a[l:r]"),
    ];

    for case in cases.iter() {
        let eqn = case.0;
        let ast = Expr0::new(eqn, LexerType::Equation).unwrap();
        assert!(ast.is_some());
        let ast = ast.unwrap().strip_loc();
        assert_eq!(&*case.1, &ast);
        let printed = print_eqn(&ast);
        assert_eq!(case.2, &printed);
    }

    let ast = Expr0::new("NAN", LexerType::Equation).unwrap();
    assert!(ast.is_some());
    let ast = ast.unwrap();
    assert!(matches!(&ast, Expr0::Const(_, _, _)));
    if let Expr0::Const(id, n, _) = &ast {
        assert_eq!("NaN", id);
        assert!(n.is_nan());
    }
    let printed = print_eqn(&ast);
    assert_eq!("NaN", &printed);
}

#[test]
fn test_parse_failures() {
    let failures = &[
        "(",
        "(3",
        "3 +",
        "3 *",
        "(3 +)",
        "call(a,",
        "call(a,1+",
        "if if",
        "if 1 then",
        "if then",
        "if 1 then 2 else",
        "a[*:2]",
        "a[2:*]",
        "a[b:*]",
        "a[*:]",
        "a[3:]",
    ];

    for case in failures {
        let err = Expr0::new(case, LexerType::Equation).unwrap_err();
        assert!(!err.is_empty());
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ast<Expr> {
    Scalar(Expr),
    ApplyToAll(Vec<Dimension>, Expr),
    Arrayed(Vec<Dimension>, HashMap<ElementName, Expr>),
}

impl Ast<Expr> {
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

pub(crate) fn lower_ast(scope: &ScopeStage0, ast: Ast<Expr0>) -> EquationResult<Ast<Expr>> {
    match ast {
        Ast::Scalar(expr) => Expr::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(Ast::Scalar),
        Ast::ApplyToAll(dims, expr) => Expr::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(|expr| Ast::ApplyToAll(dims, expr)),
        Ast::Arrayed(dims, elements) => {
            let elements: EquationResult<HashMap<ElementName, Expr>> = elements
                .into_iter()
                .map(|(id, expr)| {
                    match Expr::from(expr).map(|expr| expr.constify_dimensions(scope)) {
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

fn paren_if_necessary1(parent: &Expr, child: &Expr, eqn: String) -> String {
    if child_needs_parens!(Expr, parent, child, eqn) {
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
    fn walk_index(&mut self, expr: &IndexExpr) -> String {
        match expr {
            IndexExpr::Wildcard(_) => "*".to_string(),
            IndexExpr::StarRange(id, _) => format!("*:{}", id),
            IndexExpr::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{{NaN}}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr::Var(id, _) => {
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{}}}", id)
            }
            Expr::App(builtin, _) => {
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
            Expr::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr::Op1(op, l, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "\\neg ",
                };
                format!("{}{}", op, l)
            }
            Expr::Op2(op, l, r, _) => {
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
            Expr::If(cond, t, f, _) => {
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

pub fn latex_eqn(expr: &Expr) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_latex_eqn() {
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "(\\mathrm{a\\_c} - 1) \\cdot \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Op2(
                BinaryOp::Sub,
                Box::new(Expr::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Box::new(Expr::Op2(
                BinaryOp::Sub,
                Box::new(Expr::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-\\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Negative,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Not,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Positive,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        latex_eqn(&Expr::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "\\operatorname{lookup}(\\mathrm{a}, 1.0)",
        latex_eqn(&Expr::App(
            BuiltinFn::Lookup(
                "a".to_string(),
                Box::new(Expr::Const("1.0".to_owned(), 1.0, Default::default())),
                Default::default(),
            ),
            Loc::new(0, 14),
        ))
    );
}
