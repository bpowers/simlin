// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::literal::Literal;
use crate::builtins::{Loc, UntypedBuiltinFn, is_0_arity_builtin_fn_ci};
use crate::common::{EquationError, RawIdent};
use crate::lexer::LexerType;
use std::result::Result as StdResult;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
    Transpose,
}

/// BinaryOp enumerates the different operators supported in
/// system dynamics equations.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Hash, Copy, Clone)]
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
    ///
    /// This is the XMILE 3.3.1 table (which is also Vensim's), and it must stay
    /// in lockstep with `crate::parser`'s recursive-descent chain: each level
    /// below corresponds to one `parse_*` function, and the printers
    /// (`ast::print_eqn`, `mdl::writer`) decide parenthesization by comparing
    /// these numbers. Every operator here is left-associative EXCEPT `Exp`,
    /// which is right-to-left; the printers special-case that rather than
    /// encoding it in the table.
    pub(crate) fn precedence(&self) -> u8 {
        match self {
            BinaryOp::Or => 1,
            BinaryOp::And => 2,
            BinaryOp::Eq => 3,
            BinaryOp::Neq => 3,
            BinaryOp::Gt => 4,
            BinaryOp::Lt => 4,
            BinaryOp::Gte => 4,
            BinaryOp::Lte => 4,
            BinaryOp::Add => 5,
            BinaryOp::Sub => 5,
            BinaryOp::Mul => 6,
            BinaryOp::Div => 6,
            BinaryOp::Mod => 6,
            BinaryOp::Exp => 7,
        }
    }
}

/// Expr0 represents a parsed equation, before any calls to
/// builtin functions have been checked/resolved.
///
/// The `Eq` derive is load-bearing, not decoration: `Expr0` rides on
/// salsa-cached values (`db::query::ParsedVariableResult`, `db::ltm::LtmArm`)
/// whose backdating is decided by comparing an old value
/// with a rebuilt one, so a variant that is not equal to ITSELF permanently
/// defeats that comparison. A bare `f64` is exactly such a field (`NaN !=
/// NaN`), and `Eq` rejects it at compile time -- which is why the literal is an
/// [`Literal`], compared by bit pattern. The same argument applies to `Expr1`,
/// `Expr2` and `Expr3`; see [`Literal`] for the full statement.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum Expr0 {
    Const(String, Literal, Loc),
    Var(RawIdent, Loc),
    App(UntypedBuiltinFn<Expr0>, Loc),
    Subscript(RawIdent, Vec<IndexExpr0>, Loc),
    Op1(UnaryOp, Box<Expr0>, Loc),
    Op2(BinaryOp, Box<Expr0>, Box<Expr0>, Loc),
    If(Box<Expr0>, Box<Expr0>, Box<Expr0>, Loc),
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum IndexExpr0 {
    Wildcard(Loc),
    StarRange(RawIdent, Loc),
    Range(Expr0, Expr0, Loc),
    DimPosition(u32, Loc),
    Expr(Expr0),
}

impl IndexExpr0 {
    fn reify_0_arity_builtins(self) -> Self {
        match self {
            IndexExpr0::Wildcard(_) => self,
            IndexExpr0::StarRange(_, _) => self,
            IndexExpr0::Range(_, _, _) => self,
            IndexExpr0::DimPosition(_, _) => self,
            IndexExpr0::Expr(expr) => IndexExpr0::Expr(expr.reify_0_arity_builtins()),
        }
    }

    /// The [`Expr0::eq_ignoring_loc`] twin for one subscript index.
    pub(crate) fn eq_ignoring_loc(&self, other: &IndexExpr0) -> bool {
        match (self, other) {
            (IndexExpr0::Wildcard(_), IndexExpr0::Wildcard(_)) => true,
            (IndexExpr0::StarRange(l, _), IndexExpr0::StarRange(r, _)) => l == r,
            (IndexExpr0::Range(ll, lr, _), IndexExpr0::Range(rl, rr, _)) => {
                ll.eq_ignoring_loc(rl) && lr.eq_ignoring_loc(rr)
            }
            (IndexExpr0::DimPosition(l, _), IndexExpr0::DimPosition(r, _)) => l == r,
            (IndexExpr0::Expr(l), IndexExpr0::Expr(r)) => l.eq_ignoring_loc(r),
            _ => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            IndexExpr0::Wildcard(_loc) => IndexExpr0::Wildcard(loc),
            IndexExpr0::StarRange(d, _loc) => IndexExpr0::StarRange(d, loc),
            IndexExpr0::Range(l, r, _loc) => IndexExpr0::Range(l.strip_loc(), r.strip_loc(), loc),
            IndexExpr0::DimPosition(n, _loc) => IndexExpr0::DimPosition(n, loc),
            IndexExpr0::Expr(e) => IndexExpr0::Expr(e.strip_loc()),
        }
    }
}

impl Expr0 {
    /// Returns the source location of the first reference to the given
    /// canonical identifier, or None if the identifier is not referenced.
    pub(crate) fn get_var_loc(&self, canonical_ident: &str) -> Option<Loc> {
        match self {
            Expr0::Const(..) => None,
            Expr0::Var(raw, loc) => {
                if crate::common::canonicalize(raw.as_str()).as_ref() == canonical_ident {
                    Some(*loc)
                } else {
                    None
                }
            }
            Expr0::App(UntypedBuiltinFn(_, args), _) => {
                for arg in args {
                    if let Some(loc) = arg.get_var_loc(canonical_ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr0::Subscript(raw, indices, loc) => {
                if crate::common::canonicalize(raw.as_str()).as_ref() == canonical_ident {
                    return Some(*loc);
                }
                for idx in indices {
                    if let IndexExpr0::Range(l, r, _) = idx {
                        if let Some(loc) = l.get_var_loc(canonical_ident) {
                            return Some(loc);
                        }
                        if let Some(loc) = r.get_var_loc(canonical_ident) {
                            return Some(loc);
                        }
                    }
                }
                None
            }
            Expr0::Op1(_, inner, _) => inner.get_var_loc(canonical_ident),
            Expr0::Op2(_, l, r, _) => l
                .get_var_loc(canonical_ident)
                .or_else(|| r.get_var_loc(canonical_ident)),
            Expr0::If(cond, t, f, _) => cond
                .get_var_loc(canonical_ident)
                .or_else(|| t.get_var_loc(canonical_ident))
                .or_else(|| f.get_var_loc(canonical_ident)),
        }
    }

    /// new returns a new Expression AST if one can be constructed, or a list of
    /// source/equation errors if one couldn't be constructed.
    pub fn new(eqn: &str, lexer_type: LexerType) -> StdResult<Option<Expr0>, Vec<EquationError>> {
        match crate::parser::parse(eqn, lexer_type) {
            Ok(Some(ast)) => Ok(Some(match lexer_type {
                // in variable equations we want to treat `pi` or `time`
                // as calls to `pi()` or `time()` builtin functions.  But
                // in unit equations we might have a unit called "time", and
                // function calls don't make sense there anyway.  So only
                // reify for definitions/equations.
                LexerType::Equation => ast.reify_0_arity_builtins(),
                LexerType::Units => ast,
            })),
            Ok(None) => Ok(None),
            Err(errs) => Err(errs),
        }
    }

    /// reify turns variable references to known 0-arity builtin functions
    /// like `pi()` into App()s of those functions.
    fn reify_0_arity_builtins(self) -> Self {
        match self {
            Expr0::Var(ref id, loc) => {
                // Allocation-free membership test first: the vast majority of
                // variable references are not 0-arity builtins, so we avoid the
                // per-reference to_lowercase() heap allocation on the hot parse
                // path and only materialize the lowercased name in the rare case
                // a genuine `pi`/`time`/etc. reference must be reified.
                if is_0_arity_builtin_fn_ci(id.as_str()) {
                    let lowercase_id = id.as_str().to_lowercase();
                    Expr0::App(UntypedBuiltinFn(lowercase_id, vec![]), loc)
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

    /// Do these two expressions say the same thing, ignoring where they were
    /// written?
    ///
    /// `PartialEq` compares source positions, and it must: salsa uses it to
    /// decide whether a re-parse changed anything, and an expression that moved
    /// changes every diagnostic span derived from it. But identity of a
    /// SYNTHESIZED helper is a different question -- two helpers claiming one
    /// name are the same helper when they compute the same thing, wherever the
    /// two copies came from. The apply-to-all expansion walks one cloned body
    /// per element, and the dt and initial passes walk one equation twice, so
    /// that question is asked on every model with a capture in an arrayed
    /// equation. See [`crate::capture::Capture::same_definition`].
    ///
    /// A constant is the VALUE it denotes: `2` and `2.0` compute the same thing,
    /// so a helper minted from each is one helper. Its spelling matters only to
    /// the printer and the diagnostics, which `PartialEq` still covers.
    pub(crate) fn eq_ignoring_loc(&self, other: &Expr0) -> bool {
        match (self, other) {
            (Expr0::Const(_, ln, _), Expr0::Const(_, rn, _)) => ln == rn,
            (Expr0::Var(l, _), Expr0::Var(r, _)) => l == r,
            (
                Expr0::App(UntypedBuiltinFn(lf, largs), _),
                Expr0::App(UntypedBuiltinFn(rf, rargs), _),
            ) => {
                lf == rf
                    && largs.len() == rargs.len()
                    && largs.iter().zip(rargs).all(|(l, r)| l.eq_ignoring_loc(r))
            }
            (Expr0::Subscript(lid, lidx, _), Expr0::Subscript(rid, ridx, _)) => {
                lid == rid
                    && lidx.len() == ridx.len()
                    && lidx.iter().zip(ridx).all(|(l, r)| l.eq_ignoring_loc(r))
            }
            (Expr0::Op1(lop, l, _), Expr0::Op1(rop, r, _)) => lop == rop && l.eq_ignoring_loc(r),
            (Expr0::Op2(lop, ll, lr, _), Expr0::Op2(rop, rl, rr, _)) => {
                lop == rop && ll.eq_ignoring_loc(rl) && lr.eq_ignoring_loc(rr)
            }
            (Expr0::If(lc, lt, lf, _), Expr0::If(rc, rt, rf, _)) => {
                lc.eq_ignoring_loc(rc) && lt.eq_ignoring_loc(rt) && lf.eq_ignoring_loc(rf)
            }
            _ => false,
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
        Expr0::Const("0.0".to_string(), Literal::new(0.0), Loc::default())
    }
}

#[test]
fn test_parse() {
    use crate::ast;
    use crate::ast::BinaryOp::*;
    use Expr0::*;

    let if1 = Box::new(If(
        Box::new(Const("1".to_string(), Literal::new(1.0), Loc::default())),
        Box::new(Const("2".to_string(), Literal::new(2.0), Loc::default())),
        Box::new(Const("3".to_string(), Literal::new(3.0), Loc::default())),
        Loc::default(),
    ));

    let if2 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var(RawIdent::new_from_str("blerg"), Loc::default())),
            Box::new(Var(RawIdent::new_from_str("foo"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("2".to_string(), Literal::new(2.0), Loc::default())),
        Box::new(Const("3".to_string(), Literal::new(3.0), Loc::default())),
        Loc::default(),
    ));

    let if3 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var(RawIdent::new_from_str("quotient"), Loc::default())),
            Box::new(Var(
                RawIdent::new_from_str("quotient_target"),
                Loc::default(),
            )),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), Literal::new(1.0), Loc::default())),
        Box::new(Const("0".to_string(), Literal::new(0.0), Loc::default())),
        Loc::default(),
    ));

    let if4 = Box::new(If(
        Box::new(Op2(
            And,
            Box::new(Var(RawIdent::new_from_str("true_input"), Loc::default())),
            Box::new(Var(RawIdent::new_from_str("false_input"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), Literal::new(1.0), Loc::default())),
        Box::new(Const("0".to_string(), Literal::new(0.0), Loc::default())),
        Loc::default(),
    ));

    let quoting_eq = Box::new(Op2(
        Eq,
        Box::new(Var(RawIdent::new_from_str("\"oh dear\""), Loc::default())), // Quoted identifier with quotes
        Box::new(Var(RawIdent::new_from_str("oh_dear"), Loc::default())),
        Loc::default(),
    ));

    let subscript1 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Expr(Const(
            "1".to_owned(),
            Literal::new(1.0),
            Loc::default(),
        ))],
        Loc::default(),
    ));
    let subscript2 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Expr(Const("2".to_owned(), Literal::new(2.0), Loc::default())),
            IndexExpr0::Expr(App(
                UntypedBuiltinFn(
                    "int".to_owned(),
                    vec![Var(RawIdent::new_from_str("b"), Loc::default())],
                ),
                Loc::default(),
            )),
        ],
        Loc::default(),
    ));

    let subscript3 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Wildcard(Loc::default()),
            IndexExpr0::Wildcard(Loc::default()),
        ],
        Loc::default(),
    ));

    let subscript4 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::StarRange(
            RawIdent::new_from_str("d"),
            Loc::default(),
        )],
        Loc::default(),
    ));

    let subscript5 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Range(
            Const("1".to_owned(), Literal::new(1.0), Loc::default()),
            Const("2".to_owned(), Literal::new(2.0), Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    ));

    let subscript6 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Range(
            Var(RawIdent::new_from_str("l"), Loc::default()),
            Var(RawIdent::new_from_str("r"), Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    ));

    let dimension_pos1 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::DimPosition(1, Loc::default())],
        Loc::default(),
    ));

    let dimension_pos2 = Box::new(Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Expr(Var(RawIdent::new_from_str("DimM"), Loc::default())),
            IndexExpr0::DimPosition(1, Loc::default()),
            IndexExpr0::DimPosition(2, Loc::default()),
        ],
        Loc::default(),
    ));

    let time1 = Box::new(App(
        UntypedBuiltinFn("time".to_owned(), vec![]),
        Loc::default(),
    ));

    let time2 = Box::new(Subscript(
        RawIdent::new_from_str("aux"),
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
                        Box::new(Const("5".to_owned(), Literal::new(5.0), Loc::default())),
                        Loc::default(),
                    )],
                ),
                Loc::default(),
            )),
            Box::new(Const("1".to_owned(), Literal::new(1.0), Loc::default())),
            Loc::default(),
        ))],
        Loc::default(),
    ));

    // Test cases for transpose operator
    let transpose1 = Box::new(Op1(
        UnaryOp::Transpose,
        Box::new(Var(RawIdent::new_from_str("a"), Loc::default())),
        Loc::default(),
    ));

    let transpose2 = Box::new(Op1(
        UnaryOp::Transpose,
        Box::new(Subscript(
            RawIdent::new_from_str("matrix"),
            vec![
                IndexExpr0::Wildcard(Loc::default()),
                IndexExpr0::Expr(Const("1".to_owned(), Literal::new(1.0), Loc::default())),
            ],
            Loc::default(),
        )),
        Loc::default(),
    ));

    let transpose3 = Box::new(Op2(
        BinaryOp::Mul,
        Box::new(Op1(
            UnaryOp::Transpose,
            Box::new(Var(RawIdent::new_from_str("a"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Var(RawIdent::new_from_str("b"), Loc::default())),
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
        ("a'", transpose1, "a'"),
        ("matrix[*, 1]'", transpose2, "matrix[*, 1]'"),
        ("a' * b", transpose3, "a' * b"),
        ("a[@1]", dimension_pos1, "a[@1]"),
        ("a[DimM, @1, @2]", dimension_pos2, "a[dimm, @1, @2]"),
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
        assert!(n.value().is_nan());
    }
    let printed = ast::print_eqn(&ast);
    assert_eq!("NaN", &printed);
}

#[test]
fn test_dimension_position() {
    use crate::ast;

    // Test valid dimension positions
    let result = Expr0::new("a[@1]", LexerType::Equation);
    assert!(result.is_ok());
    let ast = result.unwrap().unwrap();
    let printed = ast::print_eqn(&ast);
    assert_eq!("a[@1]", &printed);

    // Test multiple dimension positions
    let result = Expr0::new("a[@3, @2, @1]", LexerType::Equation);
    assert!(result.is_ok());
    let ast = result.unwrap().unwrap();
    let printed = ast::print_eqn(&ast);
    assert_eq!("a[@3, @2, @1]", &printed);

    // Test mixed subscripts
    let result = Expr0::new("a[i, @1, j]", LexerType::Equation);
    assert!(result.is_ok());
    let ast = result.unwrap().unwrap();
    let printed = ast::print_eqn(&ast);
    assert_eq!("a[i, @1, j]", &printed);

    // Test large dimension position
    let result = Expr0::new("a[@100]", LexerType::Equation);
    assert!(result.is_ok());
    let ast = result.unwrap().unwrap();
    let printed = ast::print_eqn(&ast);
    assert_eq!("a[@100]", &printed);

    // Test that @0 parses correctly (validation happens at a later stage)
    let result = Expr0::new("a[@0]", LexerType::Equation);
    assert!(result.is_ok());
    let ast = result.unwrap().unwrap();
    let printed = ast::print_eqn(&ast);
    assert_eq!("a[@0]", &printed);
}

#[test]
fn test_safediv_operator() {
    use crate::ast;
    use Expr0::*;

    // a // b should parse as safediv(a, b)
    let safediv1 = Box::new(App(
        UntypedBuiltinFn(
            "safediv".to_owned(),
            vec![
                Var(RawIdent::new_from_str("a"), Loc::default()),
                Var(RawIdent::new_from_str("b"), Loc::default()),
            ],
        ),
        Loc::default(),
    ));

    // 1 // 2 should parse as safediv(1, 2)
    let safediv2 = Box::new(App(
        UntypedBuiltinFn(
            "safediv".to_owned(),
            vec![
                Const("1".to_owned(), Literal::new(1.0), Loc::default()),
                Const("2".to_owned(), Literal::new(2.0), Loc::default()),
            ],
        ),
        Loc::default(),
    ));

    // a * b // c should be safediv(a * b, c) because // has same precedence as * and is left-associative
    let safediv3 = Box::new(App(
        UntypedBuiltinFn(
            "safediv".to_owned(),
            vec![
                Op2(
                    BinaryOp::Mul,
                    Box::new(Var(RawIdent::new_from_str("a"), Loc::default())),
                    Box::new(Var(RawIdent::new_from_str("b"), Loc::default())),
                    Loc::default(),
                ),
                Var(RawIdent::new_from_str("c"), Loc::default()),
            ],
        ),
        Loc::default(),
    ));

    // a + b // c should be a + safediv(b, c) because // binds tighter than +
    let safediv4 = Box::new(Op2(
        BinaryOp::Add,
        Box::new(Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(App(
            UntypedBuiltinFn(
                "safediv".to_owned(),
                vec![
                    Var(RawIdent::new_from_str("b"), Loc::default()),
                    Var(RawIdent::new_from_str("c"), Loc::default()),
                ],
            ),
            Loc::default(),
        )),
        Loc::default(),
    ));

    let cases = [
        ("a // b", safediv1, "safediv(a, b)"),
        ("1 // 2", safediv2, "safediv(1, 2)"),
        ("a * b // c", safediv3, "safediv(a * b, c)"),
        ("a + b // c", safediv4, "a + safediv(b, c)"),
    ];

    for case in cases.iter() {
        let eqn = case.0;
        let ast = Expr0::new(eqn, LexerType::Equation).unwrap();
        assert!(ast.is_some(), "Failed to parse: {}", eqn);
        let ast = ast.unwrap().strip_loc();
        assert_eq!(&*case.1, &ast, "AST mismatch for: {}", eqn);
        let printed = ast::print_eqn(&ast);
        assert_eq!(case.2, &printed, "Print mismatch for: {}", eqn);
    }
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
