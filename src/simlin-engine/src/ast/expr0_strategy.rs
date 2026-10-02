// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Generated `Expr0` trees for the property tests over the printer and the
//! canonical form: every operator, every index form, subscripts of one to
//! four indices, calls of zero to three arguments under names in any case or
//! spaced (as other code than the parser may build them), the number
//! spellings the lexer reads, and a name pool that reaches every clause of
//! [`needs_quoting`](super::needs_quoting).

use proptest::prelude::*;

use super::{BinaryOp, Expr0, IndexExpr0, Literal, Loc, UnaryOp};
use crate::builtins::UntypedBuiltinFn;
use crate::common::RawIdent;

/// Identifiers reaching every clause of `needs_quoting`, so the round-trip
/// property is sensitive to the quoting decision and not only to operator
/// placement:
///
/// - bare-legal names, one with capitals;
/// - all eight equation-language KEYWORDS (GH #976);
/// - a leading-digit name (`1stock`);
/// - a `·` module-qualified name (`XID_Continue`, so it stays bare);
/// - the zero-argument builtin names, which a tree holds as a VARIABLE
///   reference only when the source quoted them;
/// - names as the parser hands them over when the source quoted them, quotes
///   attached: a name with a space, a literal period, a keyword, and builtin
///   names in two spellings.
///
/// Spelled out rather than read from `lexer::KEYWORDS` or the builtin table:
/// a fixture derived from the table under test could not notice that table
/// losing an entry.
pub(crate) const NAME_POOL: [&str; 30] = [
    "a",
    "b",
    "_c",
    "Capital",
    "if",
    "then",
    "else",
    "not",
    "mod",
    "and",
    "or",
    "nan",
    "1stock",
    "m·out",
    "pi",
    "inf",
    "time",
    "time_step",
    "dt",
    "initial_time",
    "starttime",
    "final_time",
    "stoptime",
    "\"quoted name\"",
    "\"a.b\"",
    "\"if\"",
    "\"pi\"",
    "\"Time\"",
    "\"dt\"",
    "\"Time Step\"",
];

/// Number spellings the lexer reads as one number each, an overflow to
/// infinity, a subnormal and the NaN literal among them.
const NUMBERS: [&str; 12] = [
    "0",
    "1",
    "2.5",
    ".5",
    "5.",
    "1e5",
    "1E-5",
    "6e+05",
    "1e-320",
    "1e400",
    "0.30000000000000004",
    "NaN",
];

/// The zero-argument builtins as the parser reads a bare reference to one: a
/// call. All nine names, aliases included; spelled out for the reason
/// [`NAME_POOL`] gives.
const NULLARY_CALLS: [&str; 9] = [
    "pi",
    "inf",
    "time",
    "time_step",
    "dt",
    "initial_time",
    "starttime",
    "final_time",
    "stoptime",
];

/// Functions of one to three arguments: builtins, a module function, a
/// multi-word name as the parser joins it, the call the `//` operator parses
/// to, and names the parser never produces (it lowercases a call's name and
/// joins a multi-word one with `_`) but a tree built otherwise may hold, in
/// capitals, mixed case and spaced.
const FUNCTIONS: [&str; 10] = [
    "abs",
    "max",
    "lookup",
    "modulo",
    "smth1",
    "vector_elm_map",
    "safediv",
    "MAX",
    "Abs",
    "Vector Elm Map",
];

/// Every `BinaryOp`, so a new variant cannot be silently skipped: the match is
/// exhaustive and adding a variant is a compile error here.
pub(crate) fn all_binary_ops() -> Vec<BinaryOp> {
    use BinaryOp::*;
    let all = [
        Add, Sub, Mul, Div, Mod, Exp, Gt, Lt, Gte, Lte, Eq, Neq, And, Or,
    ];
    for op in all {
        match op {
            Add | Sub | Mul | Div | Mod | Exp | Gt | Lt | Gte | Lte | Eq | Neq | And | Or => {}
        }
    }
    all.to_vec()
}

fn name() -> impl Strategy<Value = RawIdent> {
    prop::sample::select(&NAME_POOL[..]).prop_map(RawIdent::new_from_str)
}

fn number() -> impl Strategy<Value = Expr0> {
    prop::sample::select(&NUMBERS[..]).prop_map(|text| {
        let value: f64 = text
            .parse()
            .expect("every number spelling parses as an f64");
        Expr0::Const(text.to_string(), Literal::new(value), Loc::default())
    })
}

/// Trees over the whole of `Expr0`: each variant, each `IndexExpr0` form.
pub(crate) fn expr_strategy() -> impl Strategy<Value = Expr0> {
    let leaf = prop_oneof![
        name().prop_map(|n| Expr0::Var(n, Loc::default())),
        number(),
        prop::sample::select(&NULLARY_CALLS[..]).prop_map(|f| Expr0::App(
            UntypedBuiltinFn(f.to_string(), Box::new([])),
            Loc::default()
        )),
    ];

    leaf.prop_recursive(5, 64, 4, |inner| {
        let bin_op = prop::sample::select(all_binary_ops());
        let un_op = prop_oneof![
            Just(UnaryOp::Negative),
            Just(UnaryOp::Positive),
            Just(UnaryOp::Not),
            Just(UnaryOp::Transpose),
        ];
        // Every index form; `the_strategy_generates_every_index_form` keeps
        // the list whole.
        let index = prop_oneof![
            Just(IndexExpr0::Wildcard(Loc::default())),
            name().prop_map(|d| IndexExpr0::StarRange(d, Loc::default())),
            (1u32..4).prop_map(|n| IndexExpr0::DimPosition(n, Loc::default())),
            (inner.clone(), inner.clone()).prop_map(|(l, r)| IndexExpr0::Range(
                Box::new(l),
                Box::new(r),
                Loc::default()
            )),
            inner.clone().prop_map(IndexExpr0::Expr),
        ];
        prop_oneof![
            (bin_op, inner.clone(), inner.clone()).prop_map(|(op, l, r)| Expr0::Op2(
                op,
                Box::new(l),
                Box::new(r),
                Loc::default()
            )),
            (un_op, inner.clone()).prop_map(|(op, l)| Expr0::Op1(op, Box::new(l), Loc::default())),
            (
                prop::sample::select(&FUNCTIONS[..]),
                prop::collection::vec(inner.clone(), 1..4)
            )
                .prop_map(|(f, args)| Expr0::App(
                    UntypedBuiltinFn(f.to_owned(), args.into()),
                    Loc::default()
                )),
            (name(), prop::collection::vec(index, 1..5))
                .prop_map(|(id, indices)| Expr0::Subscript(id, indices.into(), Loc::default())),
            (inner.clone(), inner.clone(), inner).prop_map(|(c, t, f)| Expr0::If(
                Box::new(c),
                Box::new(t),
                Box::new(f),
                Loc::default()
            )),
        ]
    })
}

/// The number of `IndexExpr0` forms, by an exhaustive match: a new form is a
/// compile error here until [`expr_strategy`]'s index list (five entries)
/// generates it.
#[test]
fn the_strategy_generates_every_index_form() {
    let forms = [
        IndexExpr0::Wildcard(Loc::default()),
        IndexExpr0::StarRange(RawIdent::new_from_str("d"), Loc::default()),
        IndexExpr0::Range(Box::default(), Box::default(), Loc::default()),
        IndexExpr0::DimPosition(1, Loc::default()),
        IndexExpr0::Expr(Expr0::default()),
    ];
    for form in &forms {
        match form {
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::Range(_, _, _)
            | IndexExpr0::DimPosition(_, _)
            | IndexExpr0::Expr(_) => {}
        }
    }
    assert_eq!(forms.len(), 5);
}
