// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::ast::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
use crate::builtins::{Loc, UntypedBuiltinFn};
use crate::common::{ErrorCode, RawIdent};
use crate::lexer::LexerType;

fn parse_eq(input: &str) -> Result<Option<Expr0>, Vec<EquationError>> {
    parse(input, LexerType::Equation)
}

// ============================================================================
// Atom parsing tests
// ============================================================================

#[test]
fn test_parse_number() {
    let ast = parse_eq("42").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Const(s, n, _) if s == "42" && n == Literal::new(42.0)));
}

#[test]
fn test_parse_float() {
    let ast = parse_eq("2.75").unwrap().unwrap();
    assert!(
        matches!(ast, Expr0::Const(s, n, _) if s == "2.75" && (n.value() - 2.75).abs() < 0.001)
    );
}

#[test]
fn test_parse_scientific_notation() {
    let ast = parse_eq("1e10").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Const(s, n, _) if s == "1e10" && n == Literal::new(1e10)));
}

#[test]
fn test_parse_nan() {
    let ast = parse_eq("NaN").unwrap().unwrap();
    if let Expr0::Const(s, n, _) = ast {
        assert_eq!(s, "NaN");
        assert!(n.value().is_nan());
    } else {
        panic!("Expected Const");
    }
}

#[test]
fn test_parse_identifier() {
    let ast = parse_eq("foo").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Var(id, _) if id.as_str() == "foo"));
}

#[test]
fn test_parse_quoted_identifier() {
    let ast = parse_eq("\"quoted name\"").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Var(id, _) if id.as_str() == "\"quoted name\""));
}

#[test]
fn test_parse_parenthesized() {
    let ast = parse_eq("(42)").unwrap().unwrap().strip_loc();
    let expected = Expr0::Const("42".to_string(), Literal::new(42.0), Loc::default());
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_empty() {
    let ast = parse_eq("").unwrap();
    assert!(ast.is_none());
}

#[test]
fn test_parse_comment_only() {
    let ast = parse_eq("{this is a comment}").unwrap();
    assert!(ast.is_none());
}

#[test]
fn test_parse_whitespace_only() {
    let ast = parse_eq("   ").unwrap();
    assert!(ast.is_none());
}

// ============================================================================
// Subscript parsing tests
// ============================================================================

#[test]
fn test_parse_subscript_simple() {
    let ast = parse_eq("a[1]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Expr(Expr0::Const(
            "1".to_string(),
            Literal::new(1.0),
            Loc::default(),
        ))],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_multiple() {
    let ast = parse_eq("a[1, 2]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Expr(Expr0::Const(
                "1".to_string(),
                Literal::new(1.0),
                Loc::default(),
            )),
            IndexExpr0::Expr(Expr0::Const(
                "2".to_string(),
                Literal::new(2.0),
                Loc::default(),
            )),
        ],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_wildcard() {
    let ast = parse_eq("a[*]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Wildcard(Loc::default())],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_multiple_wildcards() {
    let ast = parse_eq("a[*, *]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Wildcard(Loc::default()),
            IndexExpr0::Wildcard(Loc::default()),
        ],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_star_range() {
    let ast = parse_eq("a[*:dim]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::StarRange(
            RawIdent::new_from_str("dim"),
            Loc::default(),
        )],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_dot_star_range() {
    let ast = parse_eq("a[*, adult_age.*]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Wildcard(Loc::default()),
            IndexExpr0::StarRange(RawIdent::new_from_str("adult_age"), Loc::default()),
        ],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_range() {
    let ast = parse_eq("a[1:2]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Range(
            Expr0::Const("1".to_string(), Literal::new(1.0), Loc::default()),
            Expr0::Const("2".to_string(), Literal::new(2.0), Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_var_range() {
    let ast = parse_eq("a[l:r]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Range(
            Expr0::Var(RawIdent::new_from_str("l"), Loc::default()),
            Expr0::Var(RawIdent::new_from_str("r"), Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_dimension_position() {
    let ast = parse_eq("a[@1]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::DimPosition(1, Loc::default())],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_mixed_dim_positions() {
    let ast = parse_eq("a[DimM, @1, @2]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Expr(Expr0::Var(RawIdent::new_from_str("DimM"), Loc::default())),
            IndexExpr0::DimPosition(1, Loc::default()),
            IndexExpr0::DimPosition(2, Loc::default()),
        ],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_trailing_comma() {
    let ast = parse_eq("a[1,]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Expr(Expr0::Const(
            "1".to_string(),
            Literal::new(1.0),
            Loc::default(),
        ))],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_empty() {
    let ast = parse_eq("a[]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(RawIdent::new_from_str("a"), vec![], Loc::default());
    assert_eq!(ast, expected);
}

// ============================================================================
// Postfix (transpose) tests
// ============================================================================

#[test]
fn test_parse_transpose() {
    let ast = parse_eq("a'").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op1(
        UnaryOp::Transpose,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_double_transpose() {
    let ast = parse_eq("a''").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op1(
        UnaryOp::Transpose,
        Box::new(Expr0::Op1(
            UnaryOp::Transpose,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subscript_transpose() {
    let ast = parse_eq("matrix[*, 1]'").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op1(
        UnaryOp::Transpose,
        Box::new(Expr0::Subscript(
            RawIdent::new_from_str("matrix"),
            vec![
                IndexExpr0::Wildcard(Loc::default()),
                IndexExpr0::Expr(Expr0::Const(
                    "1".to_string(),
                    Literal::new(1.0),
                    Loc::default(),
                )),
            ],
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Function call tests
// ============================================================================

#[test]
fn test_parse_function_call_no_args() {
    let ast = parse_eq("func()").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(UntypedBuiltinFn("func".to_string(), vec![]), Loc::default());
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_function_call_one_arg() {
    let ast = parse_eq("abs(x)").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "abs".to_string(),
            vec![Expr0::Var(RawIdent::new_from_str("x"), Loc::default())],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_function_call_multiple_args() {
    let ast = parse_eq("MAX(a, b)").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "max".to_string(),
            vec![
                Expr0::Var(RawIdent::new_from_str("a"), Loc::default()),
                Expr0::Var(RawIdent::new_from_str("b"), Loc::default()),
            ],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_function_call_trailing_comma() {
    let ast = parse_eq("func(a,)").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "func".to_string(),
            vec![Expr0::Var(RawIdent::new_from_str("a"), Loc::default())],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_nested_function_calls() {
    let ast = parse_eq("MAX(MIN(a, b), c)").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "max".to_string(),
            vec![
                Expr0::App(
                    UntypedBuiltinFn(
                        "min".to_string(),
                        vec![
                            Expr0::Var(RawIdent::new_from_str("a"), Loc::default()),
                            Expr0::Var(RawIdent::new_from_str("b"), Loc::default()),
                        ],
                    ),
                    Loc::default(),
                ),
                Expr0::Var(RawIdent::new_from_str("c"), Loc::default()),
            ],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Binary operator tests
// ============================================================================

#[test]
fn test_parse_addition() {
    let ast = parse_eq("a + b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Add,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_subtraction() {
    let ast = parse_eq("a - b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Sub,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_multiplication() {
    let ast = parse_eq("a * b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Mul,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_division() {
    let ast = parse_eq("a / b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Div,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_safe_division() {
    let ast = parse_eq("a // b").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "safediv".to_string(),
            vec![
                Expr0::Var(RawIdent::new_from_str("a"), Loc::default()),
                Expr0::Var(RawIdent::new_from_str("b"), Loc::default()),
            ],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// Note: The lexer doesn't support '%' as a character - only the keyword "mod" produces Token::Mod
// So `a % b` is NOT valid syntax in this language. Use `a mod b` instead.

#[test]
fn test_parse_modulo_keyword() {
    let ast = parse_eq("a mod b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Mod,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_exponentiation() {
    let ast = parse_eq("a ^ b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Exp,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_exponentiation_right_associative() {
    // 2^3^4 parses as 2^(3^4): `^` associates right-to-left (XMILE 3.3.1), as
    // in Vensim, Stella, and this crate's own MDL reader.
    let ast = parse_eq("2^3^4").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Exp,
        Box::new(Expr0::Const(
            "2".to_string(),
            Literal::new(2.0),
            Loc::default(),
        )),
        Box::new(Expr0::Op2(
            BinaryOp::Exp,
            Box::new(Expr0::Const(
                "3".to_string(),
                Literal::new(3.0),
                Loc::default(),
            )),
            Box::new(Expr0::Const(
                "4".to_string(),
                Literal::new(4.0),
                Loc::default(),
            )),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Comparison operator tests
// ============================================================================

#[test]
fn test_parse_less_than() {
    let ast = parse_eq("a < b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Lt,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_less_than_equal() {
    let ast = parse_eq("a <= b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Lte,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_greater_than() {
    let ast = parse_eq("a > b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Gt,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_greater_than_equal() {
    let ast = parse_eq("a >= b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Gte,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Equality operator tests
// ============================================================================

#[test]
fn test_parse_equals() {
    let ast = parse_eq("a = b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Eq,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_not_equals() {
    let ast = parse_eq("a <> b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Neq,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Logical operator tests
// ============================================================================

#[test]
fn test_parse_and() {
    let ast = parse_eq("a && b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::And,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_and_keyword() {
    let ast = parse_eq("a and b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::And,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_or() {
    let ast = parse_eq("a || b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Or,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_or_keyword() {
    let ast = parse_eq("a or b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Or,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Unary operator tests
// ============================================================================

#[test]
fn test_parse_unary_plus() {
    let ast = parse_eq("+a").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op1(
        UnaryOp::Positive,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_unary_minus() {
    let ast = parse_eq("-a").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op1(
        UnaryOp::Negative,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// Note: The lexer doesn't support '!' as a character - only the keyword "not" produces Token::Not
// So `!a` is NOT valid syntax in this language. Use `not a` instead.

#[test]
fn test_parse_unary_not_keyword() {
    let ast = parse_eq("not a").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op1(
        UnaryOp::Not,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_double_negative_with_parens() {
    let ast = parse_eq("-(-a)").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op1(
        UnaryOp::Negative,
        Box::new(Expr0::Op1(
            UnaryOp::Negative,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

/// Helper: `Op1(op, inner)` with stripped locations.
fn op1(op: UnaryOp, inner: Expr0) -> Expr0 {
    Expr0::Op1(op, Box::new(inner), Loc::default())
}

fn var(name: &str) -> Expr0 {
    Expr0::Var(RawIdent::new_from_str(name), Loc::default())
}

fn num(s: &str, v: f64) -> Expr0 {
    Expr0::Const(s.to_string(), Literal::new(v), Loc::default())
}

/// A stacked unary minus is legal Vensim (`x = - -3`), and the MDL importer's
/// `xmile_compat` formatter prints a nested negation unparenthesized (`--x`).
/// The stored datamodel equation must therefore re-parse (#912).
#[test]
fn test_parse_stacked_unary_minus() {
    let ast = parse_eq("--a").unwrap().unwrap().strip_loc();
    assert_eq!(
        ast,
        op1(UnaryOp::Negative, op1(UnaryOp::Negative, var("a")))
    );
}

/// Vensim's own surface spelling: a space between the two minus signs. The
/// lexer emits two separate `Minus` tokens either way, so this and `--3` must
/// agree.
#[test]
fn test_parse_spaced_stacked_unary_minus() {
    let ast = parse_eq("- -3").unwrap().unwrap().strip_loc();
    assert_eq!(
        ast,
        op1(UnaryOp::Negative, op1(UnaryOp::Negative, num("3", 3.0)))
    );
    assert_eq!(ast, parse_eq("--3").unwrap().unwrap().strip_loc());
}

#[test]
fn test_parse_triple_unary_minus() {
    let ast = parse_eq("---a").unwrap().unwrap().strip_loc();
    assert_eq!(
        ast,
        op1(
            UnaryOp::Negative,
            op1(UnaryOp::Negative, op1(UnaryOp::Negative, var("a")))
        )
    );
}

/// The `Plus` arm recurses too: `print_eqn`/`xmile_compat` emit a unary plus
/// unparenthesized, so `-+x` and `+-x` are reachable stored equations.
#[test]
fn test_parse_mixed_unary_minus_plus() {
    assert_eq!(
        parse_eq("-+a").unwrap().unwrap().strip_loc(),
        op1(UnaryOp::Negative, op1(UnaryOp::Positive, var("a")))
    );
    assert_eq!(
        parse_eq("+-a").unwrap().unwrap().strip_loc(),
        op1(UnaryOp::Positive, op1(UnaryOp::Negative, var("a")))
    );
}

/// The `Not` arm recurses for the same reason.
#[test]
fn test_parse_stacked_not() {
    assert_eq!(
        parse_eq("not not a").unwrap().unwrap().strip_loc(),
        op1(UnaryOp::Not, op1(UnaryOp::Not, var("a")))
    );
    assert_eq!(
        parse_eq("not -a").unwrap().unwrap().strip_loc(),
        op1(UnaryOp::Not, op1(UnaryOp::Negative, var("a")))
    );
}

/// Recursing into `parse_unary` must not change precedence: a non-prefix token
/// still falls through to `parse_exponentiation`, so `^` binds tighter than a
/// leading unary minus.
#[test]
fn test_stacked_unary_does_not_change_exp_precedence() {
    assert_eq!(
        parse_eq("-x ^ 2").unwrap().unwrap().strip_loc(),
        op1(
            UnaryOp::Negative,
            Expr0::Op2(
                BinaryOp::Exp,
                Box::new(var("x")),
                Box::new(num("2", 2.0)),
                Loc::default(),
            )
        )
    );
    // The inner operand of a stacked negation likewise absorbs the `^`.
    assert_eq!(
        parse_eq("--x ^ 2").unwrap().unwrap().strip_loc(),
        op1(
            UnaryOp::Negative,
            op1(
                UnaryOp::Negative,
                Expr0::Op2(
                    BinaryOp::Exp,
                    Box::new(var("x")),
                    Box::new(num("2", 2.0)),
                    Loc::default(),
                )
            )
        )
    );
    // Multiplicative binding is unchanged: `-a * b` is `(-a) * b`.
    assert_eq!(
        parse_eq("-a * b").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Mul,
            Box::new(op1(UnaryOp::Negative, var("a"))),
            Box::new(var("b")),
            Loc::default(),
        )
    );
    // Vensim pins this: `test/test-models/tests/exponentiation/output.tab`
    // records `associativity = -2^2 = -4`, i.e. `-(2^2)`, NOT `(-2)^2 = 4`.
    // Parenthesizing the base is the only way to negate it first.
    assert_eq!(
        parse_eq("-2 ^ 2").unwrap().unwrap().strip_loc(),
        op1(
            UnaryOp::Negative,
            Expr0::Op2(
                BinaryOp::Exp,
                Box::new(num("2", 2.0)),
                Box::new(num("2", 2.0)),
                Loc::default(),
            )
        )
    );
    assert_eq!(
        parse_eq("(-2) ^ 2").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Exp,
            Box::new(op1(UnaryOp::Negative, num("2", 2.0))),
            Box::new(num("2", 2.0)),
            Loc::default(),
        )
    );
}

/// Binary minus followed by a unary minus stays a subtraction of a negation.
#[test]
fn test_binary_minus_then_unary_minus() {
    assert_eq!(
        parse_eq("2 - -3").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Sub,
            Box::new(num("2", 2.0)),
            Box::new(op1(UnaryOp::Negative, num("3", 3.0))),
            Loc::default(),
        )
    );
}

/// The exponent operand admits a unary prefix (`2 ^ -3`), which Vensim and the
/// MDL reader accept and which `print_eqn` emits unparenthesized for
/// `Op2(Exp, a, Op1(Negative, b))`. Without it the printer's own output is
/// unparseable -- the same defect class as the stacked unary (#912).
#[test]
fn test_exponent_operand_admits_unary_prefix() {
    assert_eq!(
        parse_eq("2 ^ -3").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Exp,
            Box::new(num("2", 2.0)),
            Box::new(op1(UnaryOp::Negative, num("3", 3.0))),
            Loc::default(),
        )
    );
    assert_eq!(
        parse_eq("2 ^ --3").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Exp,
            Box::new(num("2", 2.0)),
            Box::new(op1(
                UnaryOp::Negative,
                op1(UnaryOp::Negative, num("3", 3.0))
            )),
            Loc::default(),
        )
    );
}

/// `^` is RIGHT-associative: `a ^ b ^ c` is `a ^ (b ^ c)`.
///
/// Authorities, all in agreement: the XMILE spec (3.3.1, "Exponentiation (right
/// to left)"), Vensim (`test/test-models/tests/arithmetics/output.tab` records
/// `cons4^cons3^cons2 == 262144 == 4^(3^2)`, not 4096), and this repo's own MDL
/// reader (`mdl::parser::parse_power`, whose exponent recurses into
/// `parse_unary`).
#[test]
fn test_exponentiation_is_right_associative() {
    assert_eq!(
        parse_eq("a ^ b ^ c").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Exp,
            Box::new(var("a")),
            Box::new(Expr0::Op2(
                BinaryOp::Exp,
                Box::new(var("b")),
                Box::new(var("c")),
                Loc::default(),
            )),
            Loc::default(),
        )
    );
    // Parenthesizing the LEFT operand is what forces left grouping.
    assert_eq!(
        parse_eq("(a ^ b) ^ c").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Exp,
            Box::new(Expr0::Op2(
                BinaryOp::Exp,
                Box::new(var("a")),
                Box::new(var("b")),
                Loc::default(),
            )),
            Box::new(var("c")),
            Loc::default(),
        )
    );
}

/// A prefixed exponent chains right too: `a ^ -b ^ c` is `a ^ (-(b ^ c))`.
/// Pinned against Vensim: `expo[sub6] = cons2^-cons2^-cons3` is `0.917004`
/// (`2^(-(2^(-3)))`), not `64` (`(2^-2)^-3`).
#[test]
fn test_prefixed_exponent_associates_right() {
    assert_eq!(
        parse_eq("a ^ -b ^ c").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Exp,
            Box::new(var("a")),
            Box::new(op1(
                UnaryOp::Negative,
                Expr0::Op2(
                    BinaryOp::Exp,
                    Box::new(var("b")),
                    Box::new(var("c")),
                    Loc::default(),
                )
            )),
            Loc::default(),
        )
    );
}

/// The exponent binds tighter than a multiplicative operator to its right, so
/// right-associativity does not make `^` swallow the rest of the expression.
#[test]
fn test_exponent_does_not_swallow_multiplicative() {
    assert_eq!(
        parse_eq("2 ^ 3 * 4").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Op2(
                BinaryOp::Exp,
                Box::new(num("2", 2.0)),
                Box::new(num("3", 3.0)),
                Loc::default(),
            )),
            Box::new(num("4", 4.0)),
            Loc::default(),
        )
    );
    assert_eq!(
        parse_eq("2 ^ -3 * 4").unwrap().unwrap().strip_loc(),
        Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Op2(
                BinaryOp::Exp,
                Box::new(num("2", 2.0)),
                Box::new(op1(UnaryOp::Negative, num("3", 3.0))),
                Loc::default(),
            )),
            Box::new(num("4", 4.0)),
            Loc::default(),
        )
    );
}

// ============================================================================
// If-then-else tests
// ============================================================================

#[test]
fn test_parse_if_simple() {
    let ast = parse_eq("if 1 then 2 else 3").unwrap().unwrap().strip_loc();
    let expected = Expr0::If(
        Box::new(Expr0::Const(
            "1".to_string(),
            Literal::new(1.0),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "2".to_string(),
            Literal::new(2.0),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "3".to_string(),
            Literal::new(3.0),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_if_with_condition() {
    let ast = parse_eq("if a = b then 1 else 0")
        .unwrap()
        .unwrap()
        .strip_loc();
    let expected = Expr0::If(
        Box::new(Expr0::Op2(
            BinaryOp::Eq,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "1".to_string(),
            Literal::new(1.0),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "0".to_string(),
            Literal::new(0.0),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_if_parenthesized() {
    let ast = parse_eq("(if 1 then 2 else 3)")
        .unwrap()
        .unwrap()
        .strip_loc();
    let expected = Expr0::If(
        Box::new(Expr0::Const(
            "1".to_string(),
            Literal::new(1.0),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "2".to_string(),
            Literal::new(2.0),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "3".to_string(),
            Literal::new(3.0),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_parse_if_with_logical() {
    let ast = parse_eq("if a and b then 1 else 0")
        .unwrap()
        .unwrap()
        .strip_loc();
    let expected = Expr0::If(
        Box::new(Expr0::Op2(
            BinaryOp::And,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "1".to_string(),
            Literal::new(1.0),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "0".to_string(),
            Literal::new(0.0),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Operator precedence tests
// ============================================================================

#[test]
fn test_precedence_mul_over_add() {
    // a + b * c should be a + (b * c)
    let ast = parse_eq("a + b * c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Add,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_precedence_safediv() {
    // a * b // c should be safediv(a * b, c)
    let ast = parse_eq("a * b // c").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "safediv".to_string(),
            vec![
                Expr0::Op2(
                    BinaryOp::Mul,
                    Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
                    Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                    Loc::default(),
                ),
                Expr0::Var(RawIdent::new_from_str("c"), Loc::default()),
            ],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_precedence_safediv_with_add() {
    // a + b // c should be a + safediv(b, c)
    let ast = parse_eq("a + b // c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Add,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::App(
            UntypedBuiltinFn(
                "safediv".to_string(),
                vec![
                    Expr0::Var(RawIdent::new_from_str("b"), Loc::default()),
                    Expr0::Var(RawIdent::new_from_str("c"), Loc::default()),
                ],
            ),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_precedence_comparison_over_logical() {
    // a < b && c > d should be (a < b) && (c > d)
    let ast = parse_eq("a < b && c > d").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::And,
        Box::new(Expr0::Op2(
            BinaryOp::Lt,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Op2(
            BinaryOp::Gt,
            Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("d"), Loc::default())),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_precedence_transpose_over_mul() {
    // a' * b should be (a') * b
    let ast = parse_eq("a' * b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Mul,
        Box::new(Expr0::Op1(
            UnaryOp::Transpose,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Complex expression tests
// ============================================================================

#[test]
fn test_complex_time_subscript() {
    let ast = parse_eq("aux[INT(TIME MOD 5) + 1]")
        .unwrap()
        .unwrap()
        .strip_loc();
    // This would typically be reified, but we're testing raw parsing
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("aux"),
        vec![IndexExpr0::Expr(Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::App(
                UntypedBuiltinFn(
                    "int".to_string(),
                    vec![Expr0::Op2(
                        BinaryOp::Mod,
                        Box::new(Expr0::Var(RawIdent::new_from_str("TIME"), Loc::default())),
                        Box::new(Expr0::Const(
                            "5".to_string(),
                            Literal::new(5.0),
                            Loc::default(),
                        )),
                        Loc::default(),
                    )],
                ),
                Loc::default(),
            )),
            Box::new(Expr0::Const(
                "1".to_string(),
                Literal::new(1.0),
                Loc::default(),
            )),
            Loc::default(),
        ))],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

// ============================================================================
// Error tests
// ============================================================================

#[test]
fn test_error_unclosed_paren() {
    let err = parse_eq("(3").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_unclosed_bracket() {
    let err = parse_eq("a[1").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_missing_operand() {
    let err = parse_eq("3 +").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_missing_then() {
    let err = parse_eq("if 1 2").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_missing_else() {
    let err = parse_eq("if 1 then 2").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_star_range_needs_ident() {
    let err = parse_eq("a[*:2]").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_range_needs_right() {
    let err = parse_eq("a[3:]").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_star_colon_alone() {
    let err = parse_eq("a[*:]").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_wildcard_in_range_right() {
    // a[b:*] should fail because * is not a valid expr
    let err = parse_eq("a[b:*]").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_if_if() {
    let err = parse_eq("if if").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_if_then_only() {
    let err = parse_eq("if then").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_call_unclosed() {
    let err = parse_eq("call(a,").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_call_incomplete_expr() {
    let err = parse_eq("call(a, 1+").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_unclosed_comment() {
    let err = parse_eq("{unclosed comment").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::UnclosedComment);
}

#[test]
fn test_error_unclosed_quoted_ident() {
    let err = parse_eq("\"unclosed").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::UnclosedQuotedIdent);
}

// ============================================================================
// Negative shape tests (illegal compositions)
// ============================================================================

#[test]
fn test_illegal_subscript_on_function_result() {
    // f(x)[1] should fail because function results can't be subscripted
    let err = parse_eq("func(x)[1]").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_illegal_transpose_on_function_result() {
    // f(x)' should fail because function results can't be transposed
    let err = parse_eq("func(x)'").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_illegal_subscript_on_expression() {
    // (a+b)[1] should fail because expression results can't be subscripted
    let err = parse_eq("(a+b)[1]").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_illegal_if_in_binary_expr() {
    // a + if b then c else d should fail because if needs parentheses
    let err = parse_eq("a + if b then c else d").unwrap_err();
    assert!(!err.is_empty());
}

// ============================================================================
// Loc span tests
// ============================================================================

#[test]
fn test_loc_span_const() {
    let ast = parse_eq("123").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 3);
}

#[test]
fn test_loc_span_var() {
    let ast = parse_eq("abc").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 3);
}

#[test]
fn test_loc_span_binary_op() {
    let ast = parse_eq("a + b").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 5);
}

#[test]
fn test_loc_span_function_call() {
    let ast = parse_eq("max(1, 2)").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 9);
}

#[test]
fn test_loc_span_subscript() {
    let ast = parse_eq("arr[1]").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 6);
}

#[test]
fn test_loc_span_if() {
    let ast = parse_eq("if 1 then 2 else 3").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 18);
}

#[test]
fn test_loc_span_unary() {
    let ast = parse_eq("-x").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 2);
}

#[test]
fn test_loc_span_transpose() {
    let ast = parse_eq("x'").unwrap().unwrap();
    let loc = ast.get_loc();
    assert_eq!(loc.start, 0);
    assert_eq!(loc.end, 2);
}

// ============================================================================
// Additional coverage tests
// ============================================================================

#[test]
fn test_error_extra_token() {
    // Valid expression followed by extra content should fail with ExtraToken
    let err = parse_eq("1 2").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::ExtraToken);
}

#[test]
fn test_error_extra_token_after_expr() {
    let err = parse_eq("a + b c").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::ExtraToken);
}

#[test]
fn test_chained_logical_and() {
    let ast = parse_eq("a && b && c").unwrap().unwrap().strip_loc();
    // Should be ((a && b) && c) - left associative
    let expected = Expr0::Op2(
        BinaryOp::And,
        Box::new(Expr0::Op2(
            BinaryOp::And,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_chained_logical_or() {
    let ast = parse_eq("a || b || c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Or,
        Box::new(Expr0::Op2(
            BinaryOp::Or,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

/// `and` binds tighter than `or` (XMILE 3.3.1, matching Vensim and the crate's
/// own MDL reader, whose `parse_logic_and` sits inside `parse_logic_or`).
///
/// This matters beyond the direct XMILE path: `mdl::xmile_compat` re-emits an
/// MDL AST *without* parentheses and relies on this grammar to re-establish the
/// grouping. When both operators shared one left-associative level, the correct
/// MDL tree `Or(1, And(0, 0))` printed as `1 or 0 and 0` and re-parsed as
/// `And(Or(1, 0), 0)` -- a different value.
#[test]
fn test_and_binds_tighter_than_or() {
    for src in ["a or b and c", "a || b && c"] {
        let ast = parse_eq(src).unwrap().unwrap().strip_loc();
        let expected = Expr0::Op2(
            BinaryOp::Or,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Op2(
                BinaryOp::And,
                Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
                Loc::default(),
            )),
            Loc::default(),
        );
        assert_eq!(ast, expected, "parsing {src}");
    }
}

/// The mirror of [`test_and_binds_tighter_than_or`]: an `and` on the LEFT of an
/// `or` must not swallow the `or`'s right operand.
#[test]
fn test_or_does_not_bind_into_a_leading_and() {
    let ast = parse_eq("a and b or c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Or,
        Box::new(Expr0::Op2(
            BinaryOp::And,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

/// `and`/`or` sit BELOW the comparisons, so a comparison never needs parens as a
/// logical operand. (This is where `mdl::parser`'s own table disagrees with both
/// Vensim and XMILE -- see GH #914 -- but the XMILE grammar here is correct.)
#[test]
fn test_logical_ops_bind_looser_than_comparisons() {
    let ast = parse_eq("a > b and c < d").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::And,
        Box::new(Expr0::Op2(
            BinaryOp::Gt,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Op2(
            BinaryOp::Lt,
            Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("d"), Loc::default())),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_chained_equality() {
    let ast = parse_eq("a = b = c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Eq,
        Box::new(Expr0::Op2(
            BinaryOp::Eq,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_chained_comparison() {
    let ast = parse_eq("a < b < c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Lt,
        Box::new(Expr0::Op2(
            BinaryOp::Lt,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_chained_addition() {
    let ast = parse_eq("a + b + c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Add,
        Box::new(Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_chained_multiplication() {
    let ast = parse_eq("a * b * c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Mul,
        Box::new(Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_division_chain() {
    let ast = parse_eq("a / b / c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Div,
        Box::new(Expr0::Op2(
            BinaryOp::Div,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_modulo_chain() {
    let ast = parse_eq("a mod b mod c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Mod,
        Box::new(Expr0::Op2(
            BinaryOp::Mod,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_function_three_args() {
    let ast = parse_eq("clamp(a, b, c)").unwrap().unwrap().strip_loc();
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "clamp".to_string(),
            vec![
                Expr0::Var(RawIdent::new_from_str("a"), Loc::default()),
                Expr0::Var(RawIdent::new_from_str("b"), Loc::default()),
                Expr0::Var(RawIdent::new_from_str("c"), Loc::default()),
            ],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_subscript_with_expression() {
    let ast = parse_eq("a[b + 1]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Expr(Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Box::new(Expr0::Const(
                "1".to_string(),
                Literal::new(1.0),
                Loc::default(),
            )),
            Loc::default(),
        ))],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_nested_parens() {
    let ast = parse_eq("((a))").unwrap().unwrap().strip_loc();
    let expected = Expr0::Var(RawIdent::new_from_str("a"), Loc::default());
    assert_eq!(ast, expected);
}

#[test]
fn test_deeply_nested_if() {
    let ast = parse_eq("if a then (if b then 1 else 2) else 3")
        .unwrap()
        .unwrap()
        .strip_loc();
    let expected = Expr0::If(
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::If(
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Box::new(Expr0::Const(
                "1".to_string(),
                Literal::new(1.0),
                Loc::default(),
            )),
            Box::new(Expr0::Const(
                "2".to_string(),
                Literal::new(2.0),
                Loc::default(),
            )),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "3".to_string(),
            Literal::new(3.0),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_error_at_not_followed_by_number() {
    // @x should fail because @ must be followed by integer
    let err = parse_eq("a[@x]").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::ExpectedInteger);
}

#[test]
fn test_error_at_alone() {
    // @ alone should fail
    let err = parse_eq("a[@]").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::ExpectedInteger);
}

#[test]
fn test_subscript_multiple_dim_positions() {
    let ast = parse_eq("a[@1, @2, @3]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::DimPosition(1, Loc::default()),
            IndexExpr0::DimPosition(2, Loc::default()),
            IndexExpr0::DimPosition(3, Loc::default()),
        ],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_lte_operator() {
    let ast = parse_eq("a <= b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Lte,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_gte_operator() {
    let ast = parse_eq("a >= b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Gte,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_neq_operator() {
    let ast = parse_eq("a <> b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Neq,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_mixed_comparison_chain() {
    // a <= b >= c tests both Lte and Gte in the chain
    let ast = parse_eq("a <= b >= c").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Gte,
        Box::new(Expr0::Op2(
            BinaryOp::Lte,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_safe_div_chain() {
    let ast = parse_eq("a // b // c").unwrap().unwrap().strip_loc();
    // a // b // c = safediv(safediv(a, b), c)
    let expected = Expr0::App(
        UntypedBuiltinFn(
            "safediv".to_string(),
            vec![
                Expr0::App(
                    UntypedBuiltinFn(
                        "safediv".to_string(),
                        vec![
                            Expr0::Var(RawIdent::new_from_str("a"), Loc::default()),
                            Expr0::Var(RawIdent::new_from_str("b"), Loc::default()),
                        ],
                    ),
                    Loc::default(),
                ),
                Expr0::Var(RawIdent::new_from_str("c"), Loc::default()),
            ],
        ),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_scientific_negative_exponent() {
    let ast = parse_eq("1.5e-3").unwrap().unwrap();
    assert!(
        matches!(ast, Expr0::Const(s, n, _) if s == "1.5e-3" && (n.value() - 0.0015).abs() < 1e-10)
    );
}

#[test]
fn test_leading_decimal() {
    let ast = parse_eq(".5").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Const(s, n, _) if s == ".5" && (n.value() - 0.5).abs() < 1e-10));
}

#[test]
fn test_if_with_complex_condition() {
    let ast = parse_eq("if a < b and c > d then 1 else 0")
        .unwrap()
        .unwrap()
        .strip_loc();
    let expected = Expr0::If(
        Box::new(Expr0::Op2(
            BinaryOp::And,
            Box::new(Expr0::Op2(
                BinaryOp::Lt,
                Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                Loc::default(),
            )),
            Box::new(Expr0::Op2(
                BinaryOp::Gt,
                Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("d"), Loc::default())),
                Loc::default(),
            )),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "1".to_string(),
            Literal::new(1.0),
            Loc::default(),
        )),
        Box::new(Expr0::Const(
            "0".to_string(),
            Literal::new(0.0),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_unary_in_expression() {
    let ast = parse_eq("a + -b").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Add,
        Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
        Box::new(Expr0::Op1(
            UnaryOp::Negative,
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_exp_with_unary() {
    let ast = parse_eq("-a ^ b").unwrap().unwrap().strip_loc();
    // -a ^ b is -(a ^ b) because exp binds tighter than unary
    let expected = Expr0::Op1(
        UnaryOp::Negative,
        Box::new(Expr0::Op2(
            BinaryOp::Exp,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::default())),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
            Loc::default(),
        )),
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_error_star_colon_eof() {
    // *: followed by EOF should fail
    let err = parse_eq("a[*:").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::UnrecognizedEof);
}

#[test]
fn test_error_at_eof() {
    // @ followed by EOF should fail
    let err = parse_eq("a[@").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::ExpectedInteger);
}

#[test]
fn test_error_at_float() {
    // @1.5 should fail because dim position needs integer
    let err = parse_eq("a[@1.5]").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::ExpectedInteger);
}

#[test]
fn test_error_just_operator() {
    // Just an operator with no operands
    let err = parse_eq("+").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_missing_right_operand() {
    let err = parse_eq("a *").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_unexpected_keyword() {
    // 'then' as a standalone should fail
    let err = parse_eq("then").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_unexpected_rparen() {
    // Unexpected close paren
    let err = parse_eq(")").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_error_unexpected_rbracket() {
    // Unexpected close bracket
    let err = parse_eq("]").unwrap_err();
    assert!(!err.is_empty());
}

#[test]
fn test_multiple_subscripts_with_ranges() {
    let ast = parse_eq("a[1:2, 3:4]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![
            IndexExpr0::Range(
                Expr0::Const("1".to_string(), Literal::new(1.0), Loc::default()),
                Expr0::Const("2".to_string(), Literal::new(2.0), Loc::default()),
                Loc::default(),
            ),
            IndexExpr0::Range(
                Expr0::Const("3".to_string(), Literal::new(3.0), Loc::default()),
                Expr0::Const("4".to_string(), Literal::new(4.0), Loc::default()),
                Loc::default(),
            ),
        ],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}

#[test]
fn test_wildcard_simple() {
    let ast = parse_eq("a[*]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Wildcard(Loc::default())],
        Loc::default(),
    );
    assert_eq!(ast, expected);
}
