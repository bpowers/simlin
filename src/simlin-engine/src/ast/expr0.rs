// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::Ident;
use crate::builtins::{Loc, UntypedBuiltinFn, is_0_arity_builtin_fn};
use crate::common::EquationError;
use crate::token::LexerType;
use std::result::Result as StdResult;

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
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
                use lalrpop_util::ParseError;
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

impl Default for Expr0 {
    fn default() -> Self {
        Expr0::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

#[test]
fn test_parse() {
    use crate::ast;
    use crate::ast::BinaryOp::*;
    use Expr0::*;

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
        let printed = ast::print_eqn(&ast);
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
    let printed = ast::print_eqn(&ast);
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
