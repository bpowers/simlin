// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Hand-written recursive descent parser for system dynamics equations.
//!
//! This parser replaces the LALRPOP-generated parser with equivalent functionality.
//! It uses the existing lexer and produces the same AST types (Expr0, IndexExpr0).

use crate::ast::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
use crate::builtins::{Loc, UntypedBuiltinFn};
use crate::common::{EquationError, ErrorCode, RawIdent};
use crate::lexer::{Lexer, LexerType, Spanned, Token};

#[cfg(test)]
mod tests;

/// TokenKind discriminant for efficient peek comparisons without payload matching
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TokenKind {
    If,
    Then,
    Else,
    Eq,
    Neq,
    Not,
    Mod,
    Exp,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
    Plus,
    Minus,
    Mul,
    Div,
    SafeDiv,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Apostrophe,
    At,
    Nan,
    Ident,
    Num,
}

impl<'a> From<&Token<'a>> for TokenKind {
    fn from(token: &Token<'a>) -> Self {
        match token {
            Token::If => TokenKind::If,
            Token::Then => TokenKind::Then,
            Token::Else => TokenKind::Else,
            Token::Eq => TokenKind::Eq,
            Token::Neq => TokenKind::Neq,
            Token::Not => TokenKind::Not,
            Token::Mod => TokenKind::Mod,
            Token::Exp => TokenKind::Exp,
            Token::Lt => TokenKind::Lt,
            Token::Lte => TokenKind::Lte,
            Token::Gt => TokenKind::Gt,
            Token::Gte => TokenKind::Gte,
            Token::And => TokenKind::And,
            Token::Or => TokenKind::Or,
            Token::Plus => TokenKind::Plus,
            Token::Minus => TokenKind::Minus,
            Token::Mul => TokenKind::Mul,
            Token::Div => TokenKind::Div,
            Token::SafeDiv => TokenKind::SafeDiv,
            Token::LParen => TokenKind::LParen,
            Token::RParen => TokenKind::RParen,
            Token::LBracket => TokenKind::LBracket,
            Token::RBracket => TokenKind::RBracket,
            Token::Comma => TokenKind::Comma,
            Token::Colon => TokenKind::Colon,
            Token::Apostrophe => TokenKind::Apostrophe,
            Token::At => TokenKind::At,
            Token::Nan => TokenKind::Nan,
            Token::Ident(_) => TokenKind::Ident,
            Token::Num(_) => TokenKind::Num,
        }
    }
}

/// Parser state holding tokenized input
struct Parser<'input> {
    tokens: Vec<Spanned<Token<'input>>>,
    pos: usize,
}

impl<'input> Parser<'input> {
    /// Create a new parser from a lexer, collecting all tokens up front.
    /// Returns an error if the lexer produces any errors.
    fn new(lexer: Lexer<'input>) -> Result<Self, EquationError> {
        let mut tokens = Vec::new();
        for result in lexer {
            match result {
                Ok(tok) => tokens.push(tok),
                Err(e) => return Err(e),
            }
        }
        Ok(Parser { tokens, pos: 0 })
    }

    /// Peek at the current token without consuming it
    fn peek(&self) -> Option<&Spanned<Token<'input>>> {
        self.tokens.get(self.pos)
    }

    /// Peek at the kind of the current token
    fn peek_kind(&self) -> Option<TokenKind> {
        self.peek().map(|(_, tok, _)| TokenKind::from(tok))
    }

    /// Advance to the next token and return the consumed token
    fn advance(&mut self) -> Option<&Spanned<Token<'input>>> {
        if self.pos < self.tokens.len() {
            let tok = &self.tokens[self.pos];
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    /// Expect the current token to match the expected kind, returning an error if not
    fn expect(&mut self, expected: TokenKind) -> Result<&Spanned<Token<'input>>, EquationError> {
        if self.peek_kind() == Some(expected) {
            Ok(self.advance().unwrap())
        } else if let Some((start, _, end)) = self.peek() {
            Err(EquationError {
                start: *start as u16,
                end: *end as u16,
                code: ErrorCode::UnrecognizedToken,
            })
        } else {
            let pos = self.eof_position();
            Err(EquationError {
                start: pos as u16,
                end: (pos + 1) as u16,
                code: ErrorCode::UnrecognizedEof,
            })
        }
    }

    /// Get the position for EOF errors
    fn eof_position(&self) -> usize {
        if let Some((_, _, end)) = self.tokens.last() {
            *end
        } else {
            0
        }
    }

    /// Check if we've consumed all tokens
    fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// Parse an equation from the token stream.
    /// Returns Ok(None) for empty input or comment-only input.
    fn parse_equation(&mut self) -> Result<Option<Expr0>, EquationError> {
        if self.is_at_end() {
            return Ok(None);
        }

        let expr = self.parse_expr()?;

        // Check for extra tokens after the expression
        if let Some((start, _, end)) = self.peek() {
            return Err(EquationError {
                start: *start as u16,
                end: *end as u16,
                code: ErrorCode::ExtraToken,
            });
        }

        Ok(Some(expr))
    }

    /// Parse a top-level expression (includes if-then-else)
    fn parse_expr(&mut self) -> Result<Expr0, EquationError> {
        if self.peek_kind() == Some(TokenKind::If) {
            self.parse_if()
        } else {
            self.parse_logical()
        }
    }

    /// Parse if-then-else expression
    fn parse_if(&mut self) -> Result<Expr0, EquationError> {
        let (lpos, _, _) = *self.expect(TokenKind::If)?;
        let cond = self.parse_expr()?;
        self.expect(TokenKind::Then)?;
        let then_expr = self.parse_expr()?;
        self.expect(TokenKind::Else)?;
        let else_expr = self.parse_expr()?;
        let rpos = else_expr.get_loc().end as usize;
        Ok(Expr0::If(
            Box::new(cond),
            Box::new(then_expr),
            Box::new(else_expr),
            Loc::new(lpos, rpos),
        ))
    }

    /// Parse logical operators (&&, ||, and, or) - lowest precedence binary ops
    fn parse_logical(&mut self) -> Result<Expr0, EquationError> {
        let mut left = self.parse_equality()?;

        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::And) => BinaryOp::And,
                Some(TokenKind::Or) => BinaryOp::Or,
                _ => break,
            };
            self.advance();
            let right = self.parse_equality()?;
            let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
            left = Expr0::Op2(op, Box::new(left), Box::new(right), loc);
        }

        Ok(left)
    }

    /// Parse equality operators (=, <>, !=)
    fn parse_equality(&mut self) -> Result<Expr0, EquationError> {
        let mut left = self.parse_comparison()?;

        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Eq) => BinaryOp::Eq,
                Some(TokenKind::Neq) => BinaryOp::Neq,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
            left = Expr0::Op2(op, Box::new(left), Box::new(right), loc);
        }

        Ok(left)
    }

    /// Parse comparison operators (<, <=, >, >=)
    fn parse_comparison(&mut self) -> Result<Expr0, EquationError> {
        let mut left = self.parse_additive()?;

        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Lt) => BinaryOp::Lt,
                Some(TokenKind::Lte) => BinaryOp::Lte,
                Some(TokenKind::Gt) => BinaryOp::Gt,
                Some(TokenKind::Gte) => BinaryOp::Gte,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
            left = Expr0::Op2(op, Box::new(left), Box::new(right), loc);
        }

        Ok(left)
    }

    /// Parse additive operators (+, -)
    fn parse_additive(&mut self) -> Result<Expr0, EquationError> {
        let mut left = self.parse_multiplicative()?;

        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Plus) => BinaryOp::Add,
                Some(TokenKind::Minus) => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
            left = Expr0::Op2(op, Box::new(left), Box::new(right), loc);
        }

        Ok(left)
    }

    /// Parse multiplicative operators (*, /, //, %, mod)
    fn parse_multiplicative(&mut self) -> Result<Expr0, EquationError> {
        let mut left = self.parse_unary()?;

        loop {
            match self.peek_kind() {
                Some(TokenKind::Mul) => {
                    self.advance();
                    let right = self.parse_unary()?;
                    let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
                    left = Expr0::Op2(BinaryOp::Mul, Box::new(left), Box::new(right), loc);
                }
                Some(TokenKind::Div) => {
                    self.advance();
                    let right = self.parse_unary()?;
                    let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
                    left = Expr0::Op2(BinaryOp::Div, Box::new(left), Box::new(right), loc);
                }
                Some(TokenKind::SafeDiv) => {
                    self.advance();
                    let right = self.parse_unary()?;
                    let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
                    left = Expr0::App(
                        UntypedBuiltinFn("safediv".to_string(), vec![left, right]),
                        loc,
                    );
                }
                Some(TokenKind::Mod) => {
                    self.advance();
                    let right = self.parse_unary()?;
                    let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
                    left = Expr0::Op2(BinaryOp::Mod, Box::new(left), Box::new(right), loc);
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// Parse unary operators (+, -, !, not)
    fn parse_unary(&mut self) -> Result<Expr0, EquationError> {
        match self.peek_kind() {
            Some(TokenKind::Plus) => {
                let (lpos, _, _) = *self.advance().unwrap();
                let operand = self.parse_exponentiation()?;
                let rpos = operand.get_loc().end as usize;
                Ok(Expr0::Op1(
                    UnaryOp::Positive,
                    Box::new(operand),
                    Loc::new(lpos, rpos),
                ))
            }
            Some(TokenKind::Minus) => {
                let (lpos, _, _) = *self.advance().unwrap();
                let operand = self.parse_exponentiation()?;
                let rpos = operand.get_loc().end as usize;
                Ok(Expr0::Op1(
                    UnaryOp::Negative,
                    Box::new(operand),
                    Loc::new(lpos, rpos),
                ))
            }
            Some(TokenKind::Not) => {
                let (lpos, _, _) = *self.advance().unwrap();
                let operand = self.parse_exponentiation()?;
                let rpos = operand.get_loc().end as usize;
                Ok(Expr0::Op1(
                    UnaryOp::Not,
                    Box::new(operand),
                    Loc::new(lpos, rpos),
                ))
            }
            _ => self.parse_exponentiation(),
        }
    }

    /// Parse exponentiation operator (^) - left associative per current grammar
    fn parse_exponentiation(&mut self) -> Result<Expr0, EquationError> {
        let mut left = self.parse_app()?;

        while self.peek_kind() == Some(TokenKind::Exp) {
            self.advance();
            let right = self.parse_app()?;
            let loc = Loc::new(left.get_loc().start as usize, right.get_loc().end as usize);
            left = Expr0::Op2(BinaryOp::Exp, Box::new(left), Box::new(right), loc);
        }

        Ok(left)
    }

    /// Parse function application: id(args)
    fn parse_app(&mut self) -> Result<Expr0, EquationError> {
        // Check if we have an identifier followed by '('
        if self.peek_kind() == Some(TokenKind::Ident)
            && self.pos + 1 < self.tokens.len()
            && TokenKind::from(&self.tokens[self.pos + 1].1) == TokenKind::LParen
        {
            // This is a function call
            let (lpos, tok, _) = *self.advance().unwrap();
            let name = if let Token::Ident(s) = tok {
                s.to_lowercase()
            } else {
                unreachable!()
            };

            self.advance(); // consume '('
            let args = self.parse_comma_separated_exprs()?;
            let (_, _, rpos) = *self.expect(TokenKind::RParen)?;

            return Ok(Expr0::App(
                UntypedBuiltinFn(name, args),
                Loc::new(lpos, rpos),
            ));
        }

        self.parse_postfix()
    }

    /// Parse postfix transpose operator (')
    fn parse_postfix(&mut self) -> Result<Expr0, EquationError> {
        let mut expr = self.parse_subscript()?;

        while self.peek_kind() == Some(TokenKind::Apostrophe) {
            let (_, _, rpos) = *self.advance().unwrap();
            let lpos = expr.get_loc().start as usize;
            expr = Expr0::Op1(UnaryOp::Transpose, Box::new(expr), Loc::new(lpos, rpos));
        }

        Ok(expr)
    }

    /// Parse subscript: id[args]
    fn parse_subscript(&mut self) -> Result<Expr0, EquationError> {
        // Check if we have an identifier followed by '['
        if self.peek_kind() == Some(TokenKind::Ident)
            && self.pos + 1 < self.tokens.len()
            && TokenKind::from(&self.tokens[self.pos + 1].1) == TokenKind::LBracket
        {
            let (lpos, tok, _) = *self.advance().unwrap();
            let name = if let Token::Ident(s) = tok {
                RawIdent::new_from_str(s)
            } else {
                unreachable!()
            };

            self.advance(); // consume '['
            let indices = self.parse_index_exprs()?;
            let (_, _, rpos) = *self.expect(TokenKind::RBracket)?;

            return Ok(Expr0::Subscript(name, indices, Loc::new(lpos, rpos)));
        }

        self.parse_atom()
    }

    /// Parse an atomic expression (number, identifier, parenthesized expression)
    fn parse_atom(&mut self) -> Result<Expr0, EquationError> {
        match self.peek_kind() {
            Some(TokenKind::Num) => {
                let (lpos, tok, rpos) = *self.advance().unwrap();
                if let Token::Num(s) = tok {
                    match s.parse::<f64>() {
                        Ok(n) => Ok(Expr0::Const(s.to_string(), n, Loc::new(lpos, rpos))),
                        Err(_) => Err(EquationError {
                            start: lpos as u16,
                            end: rpos as u16,
                            code: ErrorCode::ExpectedNumber,
                        }),
                    }
                } else {
                    unreachable!()
                }
            }
            Some(TokenKind::Nan) => {
                let (lpos, _, rpos) = *self.advance().unwrap();
                Ok(Expr0::Const(
                    "NaN".to_string(),
                    f64::NAN,
                    Loc::new(lpos, rpos),
                ))
            }
            Some(TokenKind::Ident) => {
                let (lpos, tok, rpos) = *self.advance().unwrap();
                if let Token::Ident(s) = tok {
                    Ok(Expr0::Var(RawIdent::new_from_str(s), Loc::new(lpos, rpos)))
                } else {
                    unreachable!()
                }
            }
            Some(TokenKind::LParen) => {
                self.advance(); // consume '('
                let expr = self.parse_expr()?;
                self.expect(TokenKind::RParen)?;
                Ok(expr)
            }
            Some(_) => {
                let (start, _, end) = self.peek().unwrap();
                Err(EquationError {
                    start: *start as u16,
                    end: *end as u16,
                    code: ErrorCode::UnrecognizedToken,
                })
            }
            None => {
                let pos = self.eof_position();
                Err(EquationError {
                    start: pos as u16,
                    end: (pos + 1) as u16,
                    code: ErrorCode::UnrecognizedEof,
                })
            }
        }
    }

    /// Parse comma-separated expressions (for function arguments)
    fn parse_comma_separated_exprs(&mut self) -> Result<Vec<Expr0>, EquationError> {
        let mut exprs = Vec::new();

        // Handle empty list
        if self.peek_kind() == Some(TokenKind::RParen) {
            return Ok(exprs);
        }

        // Parse first expression
        exprs.push(self.parse_expr()?);

        // Parse remaining expressions
        while self.peek_kind() == Some(TokenKind::Comma) {
            self.advance(); // consume ','

            // Handle trailing comma
            if self.peek_kind() == Some(TokenKind::RParen) {
                break;
            }

            exprs.push(self.parse_expr()?);
        }

        Ok(exprs)
    }

    /// Parse comma-separated index expressions (for subscripts)
    fn parse_index_exprs(&mut self) -> Result<Vec<IndexExpr0>, EquationError> {
        let mut indices = Vec::new();

        // Handle empty list
        if self.peek_kind() == Some(TokenKind::RBracket) {
            return Ok(indices);
        }

        // Parse first index expression
        indices.push(self.parse_index_expr()?);

        // Parse remaining index expressions
        while self.peek_kind() == Some(TokenKind::Comma) {
            self.advance(); // consume ','

            // Handle trailing comma
            if self.peek_kind() == Some(TokenKind::RBracket) {
                break;
            }

            indices.push(self.parse_index_expr()?);
        }

        Ok(indices)
    }

    /// Parse a single index expression
    fn parse_index_expr(&mut self) -> Result<IndexExpr0, EquationError> {
        match self.peek_kind() {
            // Wildcard: *
            Some(TokenKind::Mul) => {
                let (lpos, _, rpos) = *self.advance().unwrap();

                // Check for star-range: *:ident
                if self.peek_kind() == Some(TokenKind::Colon) {
                    self.advance(); // consume ':'

                    // Must be followed by an identifier
                    if self.peek_kind() == Some(TokenKind::Ident) {
                        let (_, tok, rpos2) = *self.advance().unwrap();
                        if let Token::Ident(s) = tok {
                            return Ok(IndexExpr0::StarRange(
                                RawIdent::new_from_str(s),
                                Loc::new(lpos, rpos2),
                            ));
                        }
                    }

                    // Error: *: must be followed by identifier
                    if let Some((start, _, end)) = self.peek() {
                        return Err(EquationError {
                            start: *start as u16,
                            end: *end as u16,
                            code: ErrorCode::UnrecognizedToken,
                        });
                    } else {
                        let pos = self.eof_position();
                        return Err(EquationError {
                            start: pos as u16,
                            end: (pos + 1) as u16,
                            code: ErrorCode::UnrecognizedEof,
                        });
                    }
                }

                Ok(IndexExpr0::Wildcard(Loc::new(lpos, rpos)))
            }

            // Dimension position: @N
            Some(TokenKind::At) => {
                let (lpos, _, _) = *self.advance().unwrap();

                // Must be followed by a number
                if self.peek_kind() == Some(TokenKind::Num) {
                    let (_, tok, rpos) = *self.advance().unwrap();
                    if let Token::Num(s) = tok {
                        match s.parse::<u32>() {
                            Ok(n) => {
                                return Ok(IndexExpr0::DimPosition(n, Loc::new(lpos, rpos)));
                            }
                            Err(_) => {
                                return Err(EquationError {
                                    start: lpos as u16,
                                    end: rpos as u16,
                                    code: ErrorCode::ExpectedInteger,
                                });
                            }
                        }
                    }
                }

                // Error: @ must be followed by integer
                if let Some((start, _, end)) = self.peek() {
                    Err(EquationError {
                        start: *start as u16,
                        end: *end as u16,
                        code: ErrorCode::ExpectedInteger,
                    })
                } else {
                    let pos = self.eof_position();
                    Err(EquationError {
                        start: pos as u16,
                        end: (pos + 1) as u16,
                        code: ErrorCode::ExpectedInteger,
                    })
                }
            }

            // Regular expression or range
            _ => {
                let left = self.parse_expr()?;

                // Check for range: expr:expr
                if self.peek_kind() == Some(TokenKind::Colon) {
                    self.advance(); // consume ':'

                    let right = self.parse_expr()?;

                    let lpos = left.get_loc().start as usize;
                    let rpos = right.get_loc().end as usize;

                    Ok(IndexExpr0::Range(left, right, Loc::new(lpos, rpos)))
                } else {
                    Ok(IndexExpr0::Expr(left))
                }
            }
        }
    }
}

/// Parse an equation string into an AST.
///
/// Returns:
/// - `Ok(Some(expr))` for valid equations
/// - `Ok(None)` for empty or comment-only input
/// - `Err(error)` for parse errors
pub fn parse(input: &str, lexer_type: LexerType) -> Result<Option<Expr0>, Vec<EquationError>> {
    let lexer = Lexer::new(input, lexer_type);
    let mut parser = match Parser::new(lexer) {
        Ok(p) => p,
        Err(e) => return Err(vec![e]),
    };

    parser.parse_equation().map_err(|e| vec![e])
}
