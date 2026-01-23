// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Helper functions for the LALRPOP parser.
//!
//! These are extracted from the grammar file since LALRPOP doesn't support
//! inline function definitions.

use crate::mdl::ast::{Equation, Expr, ExprListResult, Lhs, UnaryOp};
use crate::mdl::normalizer::{NormalizerError, NormalizerErrorCode};

/// Parse a number string to f64.
///
/// Panics if the string is not a valid number. This should never happen
/// since the lexer only emits valid number tokens.
pub fn parse_number(s: &str) -> f64 {
    s.parse()
        .unwrap_or_else(|_| panic!("lexer emitted invalid number token: {:?}", s))
}

/// Create an equation from LHS and expression list.
///
/// If the expression list has a single element, creates a Regular equation.
/// If it has multiple elements that are all numeric literals (possibly with
/// unary minus), creates a NumberList equation. Returns an error on mixed
/// expression lists (xmutil would error here too).
pub fn make_equation<'input>(
    lhs: Lhs<'input>,
    exprs: ExprListResult<'input>,
) -> Result<Equation<'input>, NormalizerError> {
    match exprs {
        ExprListResult::Single(e) => Ok(Equation::Regular(lhs, e)),
        ExprListResult::Multiple(items) => {
            // Check if all items are numbers (possibly with unary minus)
            let mut numbers = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                match extract_number(item) {
                    Some(n) => numbers.push(n),
                    None => {
                        // Not all numbers - this is an error in xmutil
                        return Err(NormalizerError {
                            start: lhs.loc.start as usize,
                            end: lhs.loc.end as usize,
                            code: NormalizerErrorCode::SemanticError(format!(
                                "mixed expression list not allowed: item {} is not a numeric literal",
                                i
                            )),
                        });
                    }
                }
            }
            Ok(Equation::NumberList(lhs, numbers))
        }
    }
}

/// The sentinel value for :NA: in Vensim.
///
/// This matches xmutil's representation of :NA: as a numeric constant.
pub const NA_VALUE: f64 = -1e38;

/// Extract a number from an expression (handles constants, unary minus, and :NA:).
///
/// Vensim allows :NA: in number lists as a sentinel value, so we treat it as
/// a numeric literal with the standard NA sentinel value.
///
/// Note: Unary plus is NOT supported in number lists (matching xmutil behavior).
/// xmutil also doesn't evaluate expressions like `1+2` in number lists.
pub fn extract_number(e: &Expr<'_>) -> Option<f64> {
    match e {
        Expr::Const(n, _) => Some(*n),
        Expr::Na(_) => Some(NA_VALUE),
        Expr::Op1(UnaryOp::Negative, inner, _) => {
            if let Expr::Const(n, _) = inner.as_ref() {
                Some(-n)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::ast::{BinaryOp, Loc};
    use std::borrow::Cow;

    fn loc() -> Loc {
        Loc::new(0, 1)
    }

    fn make_lhs(name: &str) -> Lhs<'_> {
        Lhs {
            name: Cow::Borrowed(name),
            subscripts: vec![],
            except: None,
            interp_mode: None,
            loc: loc(),
        }
    }

    // ========================================================================
    // parse_number tests
    // ========================================================================

    #[test]
    fn test_parse_number_integer() {
        assert_eq!(parse_number("42"), 42.0);
    }

    #[test]
    fn test_parse_number_float() {
        assert_eq!(parse_number("2.5"), 2.5);
    }

    #[test]
    fn test_parse_number_scientific() {
        assert_eq!(parse_number("1e6"), 1_000_000.0);
        assert_eq!(parse_number("1.5e-3"), 0.0015);
    }

    #[test]
    #[should_panic(expected = "lexer emitted invalid number token")]
    fn test_parse_number_invalid() {
        parse_number("not_a_number");
    }

    // ========================================================================
    // extract_number tests
    // ========================================================================

    #[test]
    fn test_extract_number_const() {
        let expr = Expr::Const(5.0, loc());
        assert_eq!(extract_number(&expr), Some(5.0));
    }

    #[test]
    fn test_extract_number_unary_negative() {
        let inner = Expr::Const(3.0, loc());
        let expr = Expr::Op1(UnaryOp::Negative, Box::new(inner), loc());
        assert_eq!(extract_number(&expr), Some(-3.0));
    }

    #[test]
    fn test_extract_number_unary_positive_returns_none() {
        // Unary plus is NOT supported in number lists (matching xmutil behavior)
        let inner = Expr::Const(7.0, loc());
        let expr = Expr::Op1(UnaryOp::Positive, Box::new(inner), loc());
        assert_eq!(extract_number(&expr), None);
    }

    #[test]
    fn test_extract_number_nested_unary_returns_none() {
        // -(-5) is not a simple number literal
        let inner = Expr::Const(5.0, loc());
        let neg = Expr::Op1(UnaryOp::Negative, Box::new(inner), loc());
        let expr = Expr::Op1(UnaryOp::Negative, Box::new(neg), loc());
        assert_eq!(extract_number(&expr), None);
    }

    #[test]
    fn test_extract_number_variable_returns_none() {
        let expr = Expr::Var(Cow::Borrowed("x"), vec![], loc());
        assert_eq!(extract_number(&expr), None);
    }

    #[test]
    fn test_extract_number_binary_op_returns_none() {
        let left = Expr::Const(1.0, loc());
        let right = Expr::Const(2.0, loc());
        let expr = Expr::Op2(BinaryOp::Add, Box::new(left), Box::new(right), loc());
        assert_eq!(extract_number(&expr), None);
    }

    // ========================================================================
    // make_equation tests
    // ========================================================================

    #[test]
    fn test_make_equation_single_expression() {
        let lhs = make_lhs("x");
        let expr = Expr::Const(5.0, loc());
        let result = make_equation(lhs, ExprListResult::Single(expr));
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), Equation::Regular(_, _)));
    }

    #[test]
    fn test_make_equation_number_list() {
        let lhs = make_lhs("x");
        let items = vec![
            Expr::Const(1.0, loc()),
            Expr::Const(2.0, loc()),
            Expr::Const(3.0, loc()),
        ];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_ok());
        match result.unwrap() {
            Equation::NumberList(_, nums) => {
                assert_eq!(nums, vec![1.0, 2.0, 3.0]);
            }
            other => panic!("Expected NumberList, got {:?}", other),
        }
    }

    #[test]
    fn test_make_equation_number_list_with_negatives() {
        let lhs = make_lhs("x");
        let items = vec![
            Expr::Const(1.0, loc()),
            Expr::Op1(UnaryOp::Negative, Box::new(Expr::Const(2.0, loc())), loc()),
            Expr::Const(3.0, loc()),
        ];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_ok());
        match result.unwrap() {
            Equation::NumberList(_, nums) => {
                assert_eq!(nums, vec![1.0, -2.0, 3.0]);
            }
            other => panic!("Expected NumberList, got {:?}", other),
        }
    }

    #[test]
    fn test_make_equation_mixed_list_returns_error() {
        let lhs = make_lhs("x");
        let items = vec![
            Expr::Const(1.0, loc()),
            Expr::Var(Cow::Borrowed("a"), vec![], loc()), // Not a number
            Expr::Const(3.0, loc()),
        ];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err.code, NormalizerErrorCode::SemanticError(_)));
        if let NormalizerErrorCode::SemanticError(msg) = err.code {
            assert!(msg.contains("item 1"));
            assert!(msg.contains("not a numeric literal"));
        }
    }

    #[test]
    fn test_make_equation_mixed_list_first_item_non_numeric() {
        let lhs = make_lhs("x");
        let items = vec![
            Expr::Var(Cow::Borrowed("a"), vec![], loc()), // Not a number
            Expr::Const(2.0, loc()),
        ];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_err());
        let err = result.unwrap_err();
        if let NormalizerErrorCode::SemanticError(msg) = err.code {
            assert!(msg.contains("item 0"));
        }
    }

    // ========================================================================
    // :NA: handling tests
    // ========================================================================

    #[test]
    fn test_extract_number_na() {
        let expr = Expr::Na(loc());
        assert_eq!(extract_number(&expr), Some(NA_VALUE));
    }

    #[test]
    fn test_make_equation_number_list_with_na() {
        let lhs = make_lhs("x");
        let items = vec![
            Expr::Const(1.0, loc()),
            Expr::Na(loc()),
            Expr::Const(3.0, loc()),
        ];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_ok());
        match result.unwrap() {
            Equation::NumberList(_, nums) => {
                assert_eq!(nums.len(), 3);
                assert_eq!(nums[0], 1.0);
                assert_eq!(nums[1], NA_VALUE);
                assert_eq!(nums[2], 3.0);
            }
            other => panic!("Expected NumberList, got {:?}", other),
        }
    }

    #[test]
    fn test_make_equation_number_list_all_na() {
        let lhs = make_lhs("x");
        let items = vec![Expr::Na(loc()), Expr::Na(loc()), Expr::Na(loc())];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_ok());
        match result.unwrap() {
            Equation::NumberList(_, nums) => {
                assert_eq!(nums.len(), 3);
                assert!(nums.iter().all(|&n| n == NA_VALUE));
            }
            other => panic!("Expected NumberList, got {:?}", other),
        }
    }
}
