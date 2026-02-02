// Copyright 2025 The Simlin Authors. All rights reserved.
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

#[allow(dead_code)]
fn parse_units(input: &str) -> Result<Option<Expr0>, Vec<EquationError>> {
    parse(input, LexerType::Units)
}

#[allow(dead_code)]
fn strip_loc(expr: Expr0) -> Expr0 {
    expr.strip_loc()
}

/// Parse using the old LALRPOP parser for comparison.
/// Uses Units mode to get raw parsing results without reification.
fn parse_lalrpop(input: &str) -> Result<Option<Expr0>, Vec<EquationError>> {
    // Use Units mode to avoid reification of 0-arity builtins like time/pi
    // so we compare raw parsing results
    Expr0::new(input, LexerType::Units)
}

/// Parse using new parser in Units mode
fn parse_new_units(input: &str) -> Result<Option<Expr0>, Vec<EquationError>> {
    parse(input, LexerType::Units)
}

/// Compare AST Debug strings, which handles NaN comparison correctly
fn ast_debug_eq(a: &Expr0, b: &Expr0) -> bool {
    format!("{:?}", a) == format!("{:?}", b)
}

/// Compare both parsers produce the same result (AST or error).
/// Uses Units mode to get raw parsing results without reification.
fn assert_parsers_equivalent(input: &str) {
    let old_result = parse_lalrpop(input);
    let new_result = parse_new_units(input);

    match (&old_result, &new_result) {
        (Ok(Some(old_ast)), Ok(Some(new_ast))) => {
            let old_stripped = old_ast.clone().strip_loc();
            let new_stripped = new_ast.clone().strip_loc();
            // Use Debug comparison which handles NaN correctly
            assert!(
                ast_debug_eq(&old_stripped, &new_stripped),
                "AST mismatch for '{}'\nOld: {:?}\nNew: {:?}",
                input,
                old_ast,
                new_ast
            );
            // Also verify loc spans match
            assert_eq!(
                old_ast.get_loc(),
                new_ast.get_loc(),
                "Loc span mismatch for '{}'\nOld: {:?}\nNew: {:?}",
                input,
                old_ast.get_loc(),
                new_ast.get_loc()
            );
        }
        (Ok(None), Ok(None)) => {
            // Both returned None for empty/comment-only input
        }
        (Err(old_errs), Err(new_errs)) => {
            // Both returned errors - verify error positions match
            assert_eq!(
                old_errs.len(),
                new_errs.len(),
                "Error count mismatch for '{}'\nOld: {:?}\nNew: {:?}",
                input,
                old_errs,
                new_errs
            );
            if !old_errs.is_empty() {
                assert_eq!(
                    old_errs[0].start, new_errs[0].start,
                    "Error start position mismatch for '{}'\nOld: {:?}\nNew: {:?}",
                    input, old_errs, new_errs
                );
                assert_eq!(
                    old_errs[0].end, new_errs[0].end,
                    "Error end position mismatch for '{}'\nOld: {:?}\nNew: {:?}",
                    input, old_errs, new_errs
                );
            }
        }
        _ => {
            panic!(
                "Parser results differ for '{}'\nOld: {:?}\nNew: {:?}",
                input, old_result, new_result
            );
        }
    }
}

// ============================================================================
// Atom parsing tests
// ============================================================================

#[test]
fn test_parse_number() {
    let ast = parse_eq("42").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Const(s, n, _) if s == "42" && n == 42.0));
}

#[test]
fn test_parse_float() {
    let ast = parse_eq("2.75").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Const(s, n, _) if s == "2.75" && (n - 2.75).abs() < 0.001));
}

#[test]
fn test_parse_scientific_notation() {
    let ast = parse_eq("1e10").unwrap().unwrap();
    assert!(matches!(ast, Expr0::Const(s, n, _) if s == "1e10" && n == 1e10));
}

#[test]
fn test_parse_nan() {
    let ast = parse_eq("NaN").unwrap().unwrap();
    if let Expr0::Const(s, n, _) = ast {
        assert_eq!(s, "NaN");
        assert!(n.is_nan());
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
    let expected = Expr0::Const("42".to_string(), 42.0, Loc::default());
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
            1.0,
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
            IndexExpr0::Expr(Expr0::Const("1".to_string(), 1.0, Loc::default())),
            IndexExpr0::Expr(Expr0::Const("2".to_string(), 2.0, Loc::default())),
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
fn test_parse_subscript_range() {
    let ast = parse_eq("a[1:2]").unwrap().unwrap().strip_loc();
    let expected = Expr0::Subscript(
        RawIdent::new_from_str("a"),
        vec![IndexExpr0::Range(
            Expr0::Const("1".to_string(), 1.0, Loc::default()),
            Expr0::Const("2".to_string(), 2.0, Loc::default()),
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
            1.0,
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
                IndexExpr0::Expr(Expr0::Const("1".to_string(), 1.0, Loc::default())),
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
fn test_parse_exponentiation_left_associative() {
    // 2^3^4 should parse as (2^3)^4 since it's left-associative
    let ast = parse_eq("2^3^4").unwrap().unwrap().strip_loc();
    let expected = Expr0::Op2(
        BinaryOp::Exp,
        Box::new(Expr0::Op2(
            BinaryOp::Exp,
            Box::new(Expr0::Const("2".to_string(), 2.0, Loc::default())),
            Box::new(Expr0::Const("3".to_string(), 3.0, Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr0::Const("4".to_string(), 4.0, Loc::default())),
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

// Note: `--a` is NOT valid syntax in this language.
// The grammar is Unary -> "-" Exp, and Exp -> ... App -> ... Atom
// So after a unary operator, we expect an Exp (exponentiation) not another unary.
// To write double negation, you must use parentheses: `-(-a)`
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

#[test]
fn test_error_double_negative_without_parens() {
    let err = parse_eq("--a").unwrap_err();
    assert!(!err.is_empty());
    assert_eq!(err[0].code, ErrorCode::UnrecognizedToken);
}

// ============================================================================
// If-then-else tests
// ============================================================================

#[test]
fn test_parse_if_simple() {
    let ast = parse_eq("if 1 then 2 else 3").unwrap().unwrap().strip_loc();
    let expected = Expr0::If(
        Box::new(Expr0::Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Expr0::Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Expr0::Const("3".to_string(), 3.0, Loc::default())),
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
        Box::new(Expr0::Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Expr0::Const("0".to_string(), 0.0, Loc::default())),
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
        Box::new(Expr0::Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Expr0::Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Expr0::Const("3".to_string(), 3.0, Loc::default())),
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
        Box::new(Expr0::Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Expr0::Const("0".to_string(), 0.0, Loc::default())),
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
                        Box::new(Expr0::Const("5".to_string(), 5.0, Loc::default())),
                        Loc::default(),
                    )],
                ),
                Loc::default(),
            )),
            Box::new(Expr0::Const("1".to_string(), 1.0, Loc::default())),
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
// Equivalence tests - verify new parser matches LALRPOP parser exactly
// ============================================================================

#[test]
fn test_equivalence_atoms() {
    // Numbers
    assert_parsers_equivalent("42");
    assert_parsers_equivalent("3.14");
    assert_parsers_equivalent("1e10");
    assert_parsers_equivalent("1.5e-3");
    assert_parsers_equivalent(".5");
    assert_parsers_equivalent("NaN");

    // Identifiers
    assert_parsers_equivalent("foo");
    assert_parsers_equivalent("FOO");
    assert_parsers_equivalent("foo_bar");
    assert_parsers_equivalent("\"quoted name\"");
    assert_parsers_equivalent("\"oh dear\"");

    // Parenthesized
    assert_parsers_equivalent("(42)");
    assert_parsers_equivalent("((a))");
}

#[test]
fn test_equivalence_binary_ops() {
    // Arithmetic
    assert_parsers_equivalent("a + b");
    assert_parsers_equivalent("a - b");
    assert_parsers_equivalent("a * b");
    assert_parsers_equivalent("a / b");
    assert_parsers_equivalent("a // b");
    assert_parsers_equivalent("a mod b");
    assert_parsers_equivalent("a ^ b");

    // Comparison
    assert_parsers_equivalent("a < b");
    assert_parsers_equivalent("a <= b");
    assert_parsers_equivalent("a > b");
    assert_parsers_equivalent("a >= b");
    assert_parsers_equivalent("a = b");
    assert_parsers_equivalent("a <> b");

    // Logical
    assert_parsers_equivalent("a && b");
    assert_parsers_equivalent("a || b");
    assert_parsers_equivalent("a and b");
    assert_parsers_equivalent("a or b");
}

#[test]
fn test_equivalence_unary_ops() {
    assert_parsers_equivalent("+a");
    assert_parsers_equivalent("-a");
    assert_parsers_equivalent("not a");
    assert_parsers_equivalent("a'");
    assert_parsers_equivalent("a''");
}

#[test]
fn test_equivalence_function_calls() {
    assert_parsers_equivalent("func()");
    assert_parsers_equivalent("func(a)");
    assert_parsers_equivalent("func(a, b)");
    assert_parsers_equivalent("func(a, b, c)");
    assert_parsers_equivalent("func(a,)");
    assert_parsers_equivalent("MAX(a, b)");
    assert_parsers_equivalent("MAX(MIN(a, b), c)");
    assert_parsers_equivalent("INT(TIME MOD 5)");
}

#[test]
fn test_equivalence_subscripts() {
    assert_parsers_equivalent("a[1]");
    assert_parsers_equivalent("a[i]");
    assert_parsers_equivalent("a[1, 2]");
    assert_parsers_equivalent("a[1,]");
    assert_parsers_equivalent("a[]");
    assert_parsers_equivalent("a[*]");
    assert_parsers_equivalent("a[*, *]");
    assert_parsers_equivalent("a[*:dim]");
    assert_parsers_equivalent("a[@1]");
    assert_parsers_equivalent("a[@1, @2]");
    assert_parsers_equivalent("a[DimM, @1, @2]");
    assert_parsers_equivalent("a[1:2]");
    assert_parsers_equivalent("a[l:r]");
}

#[test]
fn test_equivalence_if_expressions() {
    assert_parsers_equivalent("if 1 then 2 else 3");
    assert_parsers_equivalent("if a = b then 1 else 0");
    assert_parsers_equivalent("if a and b then 1 else 0");
    assert_parsers_equivalent("(if 1 then 2 else 3)");
    assert_parsers_equivalent("IF quotient = quotient_target THEN 1 ELSE 0");
}

#[test]
fn test_equivalence_precedence() {
    assert_parsers_equivalent("a + b * c");
    assert_parsers_equivalent("a * b + c");
    assert_parsers_equivalent("a + b // c");
    assert_parsers_equivalent("a * b // c");
    assert_parsers_equivalent("a ^ b ^ c");
    assert_parsers_equivalent("a < b && c > d");
    assert_parsers_equivalent("a' * b");
}

#[test]
fn test_equivalence_complex() {
    assert_parsers_equivalent("aux[INT(TIME MOD 5) + 1]");
    assert_parsers_equivalent("matrix[*, 1]'");
    assert_parsers_equivalent("a' * b");
    assert_parsers_equivalent("\"oh dear\" = oh_dear");
    assert_parsers_equivalent("( IF true_input and false_input THEN 1 ELSE 0 )");
}

#[test]
fn test_equivalence_empty() {
    assert_parsers_equivalent("");
    assert_parsers_equivalent("   ");
    assert_parsers_equivalent("{comment}");
    assert_parsers_equivalent("{comment only}");
}

#[test]
fn test_equivalence_errors() {
    assert_parsers_equivalent("(");
    assert_parsers_equivalent("(3");
    assert_parsers_equivalent("3 +");
    assert_parsers_equivalent("call(a,");
    assert_parsers_equivalent("if 1 then");
    assert_parsers_equivalent("if then");
    assert_parsers_equivalent("a[*:2]");
    assert_parsers_equivalent("a[3:]");
    assert_parsers_equivalent("func(x)[1]");
    assert_parsers_equivalent("func(x)'");
    assert_parsers_equivalent("(a+b)[1]");
}
