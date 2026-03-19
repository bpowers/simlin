// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Line-oriented lexer for the systems format.
//!
//! Classifies each line as a comment, stock-only declaration, or flow line,
//! and tokenizes rate formulas into expression trees.
//!
//! The lexer operates in two stages:
//! 1. Line classification: split input into lines, classify each by structure
//! 2. Expression parsing: recursive descent for formula expressions

use super::ast::{BinOp, Expr};

/// A parsed stock reference with optional parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct StockRef {
    pub name: String,
    pub params: Vec<Expr>,
    pub is_infinite: bool,
}

/// The explicit flow type keyword, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitFlowType {
    Rate,
    Conversion,
    Leak,
}

/// A parsed rate expression: either explicitly typed or an implicit formula.
#[derive(Debug, Clone, PartialEq)]
pub enum RateExpr {
    /// Explicit type prefix, e.g. `Rate(5)` or `Conversion(0.5)`
    Explicit(ExplicitFlowType, Expr),
    /// Implicit type (bare formula after `@`)
    Implicit(Expr),
}

/// A classified line from the systems format input.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// A comment line (starts with `#`)
    Comment,
    /// A stock-only declaration (no `>` or `@`)
    StockOnly(StockRef),
    /// A flow line: source > dest @ rate
    Flow(StockRef, StockRef, RateExpr),
    /// A flow direction without rate: source > dest (no `@`)
    FlowNoRate(StockRef, StockRef),
}

/// Lex all non-empty, non-comment lines from the input.
pub fn lex_lines(input: &str) -> Result<Vec<Line>, LexError> {
    let mut lines = Vec::new();
    for (line_num, raw_line) in input.lines().enumerate() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            lines.push(Line::Comment);
            continue;
        }
        let line = lex_line(trimmed, line_num + 1)?;
        lines.push(line);
    }
    Ok(lines)
}

/// Lex a single non-comment, non-empty line.
fn lex_line(line: &str, line_num: usize) -> Result<Line, LexError> {
    // Check for flow direction ` > `
    if let Some(arrow_pos) = find_flow_arrow(line) {
        let source_part = line[..arrow_pos].trim();
        let rest = line[arrow_pos + 1..].trim();

        let source = lex_stock_ref(source_part, line_num)?;

        // Check for `@` delimiter in the rest
        if let Some(at_pos) = find_rate_delimiter(rest) {
            let dest_part = rest[..at_pos].trim();
            let rate_part = rest[at_pos + 1..].trim();

            let dest = lex_stock_ref(dest_part, line_num)?;
            let rate = lex_rate_expr(rate_part, line_num)?;
            Ok(Line::Flow(source, dest, rate))
        } else {
            // Flow direction without rate
            let dest = lex_stock_ref(rest, line_num)?;
            Ok(Line::FlowNoRate(source, dest))
        }
    } else {
        // Stock-only declaration
        let stock = lex_stock_ref(line, line_num)?;
        Ok(Line::StockOnly(stock))
    }
}

/// Find the position of `>` used as flow direction.
/// The `>` must be outside parentheses and brackets to distinguish
/// from `>` inside grouped expressions.
fn find_flow_arrow(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut paren_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            b'[' => bracket_depth += 1,
            b']' => bracket_depth -= 1,
            b'>' if paren_depth == 0 && bracket_depth == 0 => {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

/// Find the position of `@` used as flow delimiter.
/// Must be outside parentheses.
fn find_rate_delimiter(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut paren_depth: i32 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => paren_depth += 1,
            b')' => paren_depth -= 1,
            b'@' if paren_depth == 0 => {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

/// Parse a stock reference: `Name`, `Name(expr)`, `Name(expr, expr)`, or `[Name]`.
fn lex_stock_ref(s: &str, line_num: usize) -> Result<StockRef, LexError> {
    let s = s.trim();

    // Check for infinite stock syntax [Name]
    if s.starts_with('[') {
        if !s.ends_with(']') {
            return Err(LexError {
                line: line_num,
                message: format!("unclosed bracket in infinite stock: {}", s),
            });
        }
        let name = s[1..s.len() - 1].trim();
        validate_stock_name(name, line_num)?;
        return Ok(StockRef {
            name: name.to_owned(),
            params: Vec::new(),
            is_infinite: true,
        });
    }

    // Check for parameterized stock: Name(...)
    if let Some(paren_start) = s.find('(') {
        let name = &s[..paren_start];
        validate_stock_name(name, line_num)?;

        if !s.ends_with(')') {
            return Err(LexError {
                line: line_num,
                message: format!("unclosed parenthesis in stock parameters: {}", s),
            });
        }

        let params_str = &s[paren_start + 1..s.len() - 1];
        let params = lex_param_list(params_str, line_num)?;

        return Ok(StockRef {
            name: name.to_owned(),
            params,
            is_infinite: false,
        });
    }

    // Plain stock name
    validate_stock_name(s, line_num)?;
    Ok(StockRef {
        name: s.to_owned(),
        params: Vec::new(),
        is_infinite: false,
    })
}

/// Validate that a stock name matches `[a-zA-Z][a-zA-Z0-9_]*`.
fn validate_stock_name(name: &str, line_num: usize) -> Result<(), LexError> {
    if name.is_empty() {
        return Err(LexError {
            line: line_num,
            message: "empty stock name".to_owned(),
        });
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphabetic() {
        return Err(LexError {
            line: line_num,
            message: format!("stock name must start with a letter, got '{}'", name),
        });
    }
    for ch in chars {
        if !ch.is_ascii_alphanumeric() && ch != '_' {
            return Err(LexError {
                line: line_num,
                message: format!("invalid character '{}' in stock name '{}'", ch, name),
            });
        }
    }
    Ok(())
}

/// Parse a comma-separated parameter list, where each parameter is a formula.
fn lex_param_list(s: &str, line_num: usize) -> Result<Vec<Expr>, LexError> {
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }

    // Split on commas, respecting parentheses
    let parts = split_on_comma(s);
    let mut exprs = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            return Err(LexError {
                line: line_num,
                message: "empty parameter in parameter list".to_owned(),
            });
        }
        let expr = parse_formula(part, line_num)?;
        exprs.push(expr);
    }
    Ok(exprs)
}

/// Split a string on commas, respecting parenthesized groups.
fn split_on_comma(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut paren_depth = 0;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            ',' if paren_depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Parse a rate expression after `@`.
/// Detects explicit type prefixes (Rate, Conversion, Leak) case-insensitively.
fn lex_rate_expr(s: &str, line_num: usize) -> Result<RateExpr, LexError> {
    let s = s.trim();

    // Try to match explicit type prefix: Rate(...), Conversion(...), Leak(...)
    if let Some(result) = try_explicit_type(s, "rate", ExplicitFlowType::Rate, line_num)? {
        return Ok(result);
    }
    if let Some(result) =
        try_explicit_type(s, "conversion", ExplicitFlowType::Conversion, line_num)?
    {
        return Ok(result);
    }
    if let Some(result) = try_explicit_type(s, "leak", ExplicitFlowType::Leak, line_num)? {
        return Ok(result);
    }

    // Implicit type: bare formula
    let expr = parse_formula(s, line_num)?;
    Ok(RateExpr::Implicit(expr))
}

/// Try to match an explicit flow type prefix (case-insensitive).
fn try_explicit_type(
    s: &str,
    keyword: &str,
    flow_type: ExplicitFlowType,
    line_num: usize,
) -> Result<Option<RateExpr>, LexError> {
    let len = keyword.len();
    if s.len() <= len {
        return Ok(None);
    }
    if !s[..len].eq_ignore_ascii_case(keyword) {
        return Ok(None);
    }
    let rest = &s[len..];
    if !rest.starts_with('(') || !rest.ends_with(')') {
        return Ok(None);
    }
    let inner = &rest[1..rest.len() - 1];
    let expr = parse_formula(inner, line_num)?;
    Ok(Some(RateExpr::Explicit(flow_type, expr)))
}

/// Parse a formula string into an `Expr` tree.
/// Formulas are evaluated strictly left-to-right with no operator precedence.
///
/// Grammar (left-to-right, no precedence):
///   formula = atom (binop atom)*
///   atom    = INT | FLOAT | INF | NAME | '(' formula ')'
///   binop   = '+' | '-' | '*' | '/'
pub fn parse_formula(s: &str, line_num: usize) -> Result<Expr, LexError> {
    let tokens = tokenize_formula(s, line_num)?;
    let (expr, rest) = parse_expr(&tokens, line_num)?;
    if !rest.is_empty() {
        return Err(LexError {
            line: line_num,
            message: format!("unexpected token after formula: {:?}", rest[0]),
        });
    }
    Ok(expr)
}

/// Low-level token for formula tokenization.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    Int(i64),
    Float(f64),
    Inf,
    Name(String),
    Op(BinOp),
    LParen,
    RParen,
}

/// Tokenize a formula string into a flat token stream.
fn tokenize_formula(s: &str, line_num: usize) -> Result<Vec<Token>, LexError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];

        // Skip whitespace
        if ch.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        match ch {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '+' => {
                tokens.push(Token::Op(BinOp::Add));
                i += 1;
            }
            '*' => {
                tokens.push(Token::Op(BinOp::Mul));
                i += 1;
            }
            '/' => {
                tokens.push(Token::Op(BinOp::Div));
                i += 1;
            }
            '-' => {
                // Disambiguate unary minus (for negative integers like -5)
                // vs binary subtraction. Unary if preceded by nothing or an operator/lparen.
                let is_unary = tokens.is_empty()
                    || matches!(tokens.last(), Some(Token::Op(_)) | Some(Token::LParen));
                if is_unary && i + 1 < chars.len() && (chars[i + 1].is_ascii_digit()) {
                    // Parse negative number
                    let start = i;
                    i += 1;
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                        i += 1;
                    }
                    let num_str: String = chars[start..i].iter().collect();
                    if num_str.contains('.') {
                        let val: f64 = num_str.parse().map_err(|_| LexError {
                            line: line_num,
                            message: format!("invalid decimal: {}", num_str),
                        })?;
                        tokens.push(Token::Float(val));
                    } else {
                        let val: i64 = num_str.parse().map_err(|_| LexError {
                            line: line_num,
                            message: format!("invalid integer: {}", num_str),
                        })?;
                        tokens.push(Token::Int(val));
                    }
                } else {
                    tokens.push(Token::Op(BinOp::Sub));
                    i += 1;
                }
            }
            _ if ch.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                if num_str.contains('.') {
                    let val: f64 = num_str.parse().map_err(|_| LexError {
                        line: line_num,
                        message: format!("invalid decimal: {}", num_str),
                    })?;
                    tokens.push(Token::Float(val));
                } else {
                    let val: i64 = num_str.parse().map_err(|_| LexError {
                        line: line_num,
                        message: format!("invalid integer: {}", num_str),
                    })?;
                    tokens.push(Token::Int(val));
                }
            }
            _ if ch.is_ascii_alphabetic() || ch == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let name: String = chars[start..i].iter().collect();
                if name.eq_ignore_ascii_case("inf") {
                    tokens.push(Token::Inf);
                } else {
                    tokens.push(Token::Name(name));
                }
            }
            _ => {
                return Err(LexError {
                    line: line_num,
                    message: format!("unexpected character in formula: '{}'", ch),
                });
            }
        }
    }

    Ok(tokens)
}

/// Parse a formula expression: left-to-right with no operator precedence.
///   formula = atom (binop atom)*
fn parse_expr(tokens: &[Token], line_num: usize) -> Result<(Expr, &[Token]), LexError> {
    let (mut left, mut rest) = parse_atom(tokens, line_num)?;

    while let Some(Token::Op(op)) = rest.first() {
        let op = *op;
        let (right, remaining) = parse_atom(&rest[1..], line_num)?;
        left = Expr::BinOp(Box::new(left), op, Box::new(right));
        rest = remaining;
    }

    Ok((left, rest))
}

/// Parse an atom: literal, reference, or parenthesized expression.
fn parse_atom(tokens: &[Token], line_num: usize) -> Result<(Expr, &[Token]), LexError> {
    match tokens.first() {
        Some(Token::Int(n)) => Ok((Expr::Int(*n), &tokens[1..])),
        Some(Token::Float(f)) => Ok((Expr::Float(*f), &tokens[1..])),
        Some(Token::Inf) => Ok((Expr::Inf, &tokens[1..])),
        Some(Token::Name(name)) => Ok((Expr::Ref(name.clone()), &tokens[1..])),
        Some(Token::LParen) => {
            let (inner, rest) = parse_expr(&tokens[1..], line_num)?;
            match rest.first() {
                Some(Token::RParen) => Ok((Expr::Paren(Box::new(inner)), &rest[1..])),
                _ => Err(LexError {
                    line: line_num,
                    message: "unclosed parenthesis in formula".to_owned(),
                }),
            }
        }
        Some(tok) => Err(LexError {
            line: line_num,
            message: format!("unexpected token in formula: {:?}", tok),
        }),
        None => Err(LexError {
            line: line_num,
            message: "unexpected end of formula".to_owned(),
        }),
    }
}

/// A lexer error with line number and message.
#[derive(Debug, Clone)]
pub struct LexError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for LexError {}
