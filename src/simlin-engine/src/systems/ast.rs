// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! AST types for the systems format intermediate representation.
//!
//! The systems format is a line-oriented textual notation for stock-and-flow
//! models. This module defines the IR produced by the parser, preserving
//! declaration order (critical for sequential debiting priority).

use std::fmt;

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
