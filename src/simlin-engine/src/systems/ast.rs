// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! AST types for the systems format intermediate representation.
//!
//! The systems format is a line-oriented textual notation for stock-and-flow
//! models. This module defines the IR produced by the parser, preserving
//! declaration order (critical for sequential debiting priority).

use std::fmt;

use crate::canonicalize;

/// A formula expression in the systems format.
/// Formulas are evaluated strictly left-to-right with no operator precedence.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Integer literal (e.g., `5`)
    Int(i64),
    /// Decimal literal (e.g., `0.5`)
    Float(f64),
    /// Reference to a stock name (e.g., `Recruiters`)
    Ref(String),
    /// The `inf` literal
    Inf,
    /// Binary operation (left, op, right)
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    /// Parenthesized expression
    Paren(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    /// Standard mathematical precedence group.
    /// Add/Sub = 1, Mul/Div = 2.
    fn precedence(self) -> u8 {
        match self {
            BinOp::Add | BinOp::Sub => 1,
            BinOp::Mul | BinOp::Div => 2,
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinOp::Add => write!(f, "+"),
            BinOp::Sub => write!(f, "-"),
            BinOp::Mul => write!(f, "*"),
            BinOp::Div => write!(f, "/"),
        }
    }
}

impl Expr {
    /// Convert to an equation string with explicit parenthesization
    /// to preserve left-to-right evaluation semantics.
    ///
    /// The systems format evaluates expressions strictly left-to-right
    /// (no operator precedence), but simlin's equation parser uses standard
    /// math precedence. To preserve the left-to-right semantics, we must
    /// parenthesize the left operand of a BinOp when it has lower precedence
    /// than the outer operator (e.g., `(a + b) * c`).
    pub fn to_equation_string(&self) -> String {
        match self {
            Expr::Int(n) => format!("{n}"),
            Expr::Float(f) => {
                let s = format!("{f}");
                // Ensure decimal point is present for clarity
                if s.contains('.') { s } else { format!("{f}.0") }
            }
            Expr::Ref(name) => canonicalize(name).into_owned(),
            Expr::Inf => "inf()".to_string(),
            Expr::Paren(inner) => format!("({})", inner.to_equation_string()),
            Expr::BinOp(left, op, right) => {
                let left_str = if needs_parens(left, *op) {
                    format!("({})", left.to_equation_string())
                } else {
                    left.to_equation_string()
                };
                let right_str = right.to_equation_string();
                format!("{left_str} {op} {right_str}")
            }
        }
    }
}

/// The left operand of a BinOp needs parentheses when it is itself a BinOp
/// with lower precedence than the outer operator. This is the only case
/// where standard math precedence would disagree with left-to-right evaluation.
fn needs_parens(expr: &Expr, outer_op: BinOp) -> bool {
    match expr {
        Expr::BinOp(_, inner_op, _) => inner_op.precedence() < outer_op.precedence(),
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowType {
    Rate,
    Conversion,
    Leak,
}

impl fmt::Display for FlowType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowType::Rate => write!(f, "Rate"),
            FlowType::Conversion => write!(f, "Conversion"),
            FlowType::Leak => write!(f, "Leak"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemsStock {
    pub name: String,
    pub initial: Expr,
    pub max: Expr,
    pub is_infinite: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemsFlow {
    pub source: String,
    pub dest: String,
    pub flow_type: FlowType,
    pub rate: Expr,
}

/// The intermediate representation produced by the parser.
/// Declaration order is preserved (critical for sequential debiting priority).
#[derive(Debug, Clone, PartialEq)]
pub struct SystemsModel {
    pub stocks: Vec<SystemsStock>,
    pub flows: Vec<SystemsFlow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------
    // AC7.1: Left-to-right parenthesization preserves evaluation order
    // -------------------------------------------------------------------

    /// `a + b * c` parsed left-to-right as `(a + b) * c` must emit "(a + b) * c"
    /// because standard precedence would otherwise evaluate `b * c` first.
    #[test]
    fn ac7_1_add_then_mul_needs_parens() {
        // AST: BinOp(BinOp(Ref("a"), Add, Ref("b")), Mul, Ref("c"))
        let expr = Expr::BinOp(
            Box::new(Expr::BinOp(
                Box::new(Expr::Ref("a".to_owned())),
                BinOp::Add,
                Box::new(Expr::Ref("b".to_owned())),
            )),
            BinOp::Mul,
            Box::new(Expr::Ref("c".to_owned())),
        );
        assert_eq!(expr.to_equation_string(), "(a + b) * c");
    }

    /// `a * b + c` parsed left-to-right as `(a * b) + c` should emit "a * b + c"
    /// because standard precedence already evaluates `a * b` first.
    #[test]
    fn ac7_1_mul_then_add_no_extra_parens() {
        let expr = Expr::BinOp(
            Box::new(Expr::BinOp(
                Box::new(Expr::Ref("a".to_owned())),
                BinOp::Mul,
                Box::new(Expr::Ref("b".to_owned())),
            )),
            BinOp::Add,
            Box::new(Expr::Ref("c".to_owned())),
        );
        assert_eq!(expr.to_equation_string(), "a * b + c");
    }

    /// `a - b / c` parsed left-to-right as `(a - b) / c` needs parens.
    #[test]
    fn ac7_1_sub_then_div_needs_parens() {
        let expr = Expr::BinOp(
            Box::new(Expr::BinOp(
                Box::new(Expr::Ref("a".to_owned())),
                BinOp::Sub,
                Box::new(Expr::Ref("b".to_owned())),
            )),
            BinOp::Div,
            Box::new(Expr::Ref("c".to_owned())),
        );
        assert_eq!(expr.to_equation_string(), "(a - b) / c");
    }

    /// Same-precedence left nesting (e.g. `a + b - c`) does not need extra parens
    /// since standard math is left-associative for same-precedence ops.
    #[test]
    fn same_precedence_no_parens() {
        let expr = Expr::BinOp(
            Box::new(Expr::BinOp(
                Box::new(Expr::Ref("a".to_owned())),
                BinOp::Add,
                Box::new(Expr::Ref("b".to_owned())),
            )),
            BinOp::Sub,
            Box::new(Expr::Ref("c".to_owned())),
        );
        assert_eq!(expr.to_equation_string(), "a + b - c");
    }

    // -------------------------------------------------------------------
    // Simple expression conversions
    // -------------------------------------------------------------------

    #[test]
    fn int_literal() {
        assert_eq!(Expr::Int(5).to_equation_string(), "5");
    }

    #[test]
    fn negative_int() {
        assert_eq!(Expr::Int(-3).to_equation_string(), "-3");
    }

    #[test]
    fn float_literal() {
        assert_eq!(Expr::Float(0.5).to_equation_string(), "0.5");
    }

    #[test]
    fn float_whole_number() {
        // 1.0 should keep its decimal point for clarity
        assert_eq!(Expr::Float(1.0).to_equation_string(), "1.0");
    }

    #[test]
    fn inf_literal() {
        assert_eq!(Expr::Inf.to_equation_string(), "inf()");
    }

    #[test]
    fn reference_canonicalized() {
        // CamelCase names are lowercased, multi-word names get underscores
        assert_eq!(
            Expr::Ref("Recruiters".to_owned()).to_equation_string(),
            "recruiters"
        );
    }

    #[test]
    fn reference_multi_word() {
        assert_eq!(
            Expr::Ref("Phone Screens".to_owned()).to_equation_string(),
            "phone_screens"
        );
    }

    #[test]
    fn explicit_paren_preserved() {
        let expr = Expr::Paren(Box::new(Expr::BinOp(
            Box::new(Expr::Ref("a".to_owned())),
            BinOp::Add,
            Box::new(Expr::Ref("b".to_owned())),
        )));
        assert_eq!(expr.to_equation_string(), "(a + b)");
    }

    /// projects.txt formula: Developers / (Projects+1)
    #[test]
    fn complex_formula_with_explicit_parens() {
        let expr = Expr::BinOp(
            Box::new(Expr::Ref("Developers".to_owned())),
            BinOp::Div,
            Box::new(Expr::Paren(Box::new(Expr::BinOp(
                Box::new(Expr::Ref("Projects".to_owned())),
                BinOp::Add,
                Box::new(Expr::Int(1)),
            )))),
        );
        assert_eq!(expr.to_equation_string(), "developers / (projects + 1)");
    }

    /// Recruiters * 3 (simple reference * integer)
    #[test]
    fn reference_times_int() {
        let expr = Expr::BinOp(
            Box::new(Expr::Ref("Recruiters".to_owned())),
            BinOp::Mul,
            Box::new(Expr::Int(3)),
        );
        assert_eq!(expr.to_equation_string(), "recruiters * 3");
    }
}
