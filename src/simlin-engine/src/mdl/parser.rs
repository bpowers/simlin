// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Hand-written recursive descent parser for Vensim MDL equations.
//!
//! This parser consumes tokens from TokenNormalizer and produces AST types.
//! It replaces the previous LALRPOP grammar with identical behavior.

use std::borrow::Cow;
use std::fmt;

use crate::mdl::ast::{
    BinaryOp, CallKind, Equation, ExceptList, Expr, ExprListResult, InterpMode, Lhs, Loc,
    LookupTable, MappingEntry, SectionEnd, Subscript, SubscriptDef, SubscriptElement,
    SubscriptMapping, UnaryOp, UnitExpr, UnitRange, Units,
};
#[cfg(test)]
use crate::mdl::normalizer::NormalizerError;
use crate::mdl::normalizer::Token;

// ============================================================================
// Error type
// ============================================================================

/// Parse error with source location.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "parse error at {}..{}: {}",
            self.start, self.end, self.message
        )
    }
}

impl std::error::Error for ParseError {}

// ============================================================================
// Helpers moved from parser_helpers.rs
// ============================================================================

/// The sentinel value for :NA: in Vensim.
const NA_VALUE: f64 = -1e38;

/// Parse a number string to f64.
fn parse_number(s: &str, start: usize, end: usize) -> Result<f64, ParseError> {
    s.parse().map_err(|_| ParseError {
        start,
        end,
        message: format!("lexer emitted invalid number token: '{s}'"),
    })
}

/// Extract a number from an expression (handles constants, unary minus, and :NA:).
fn extract_number(e: &Expr<'_>) -> Option<f64> {
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

/// Create an equation from LHS and expression list.
fn make_equation<'input>(
    lhs: Lhs<'input>,
    exprs: ExprListResult<'input>,
) -> Result<Equation<'input>, ParseError> {
    match exprs {
        ExprListResult::Single(e) => Ok(Equation::Regular(lhs, e)),
        ExprListResult::Multiple(items) => {
            let mut numbers = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                match extract_number(item) {
                    Some(n) => numbers.push(n),
                    None => {
                        return Err(ParseError {
                            start: lhs.loc.start as usize,
                            end: lhs.loc.end as usize,
                            message: format!(
                                "mixed expression list not allowed: item {} is not a numeric literal",
                                i
                            ),
                        });
                    }
                }
            }
            Ok(Equation::NumberList(lhs, numbers))
        }
    }
}

// ============================================================================
// Token kind helper (for matching without extracting data)
// ============================================================================

/// Discriminant-only token kind for peek comparisons.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Plus,
    Minus,
    Mul,
    Div,
    Exp,
    Lt,
    Gt,
    Eq,
    Lte,
    Gte,
    Neq,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    Pipe,
    Tilde,
    Dot,
    Bang,
    DataEquals,
    Equiv,
    MapArrow,
    Number,
    Symbol,
    Literal,
    And,
    Or,
    Not,
    Na,
    Macro,
    EndOfMacro,
    Except,
    Interpolate,
    Raw,
    HoldBackward,
    LookForward,
    Implies,
    TestInput,
    TheCondition,
    EqEnd,
    GroupStar,
    Question,
    UnitsSymbol,
    Function,
    WithLookup,
    TabbedArray,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Mul => "*",
            TokenKind::Div => "/",
            TokenKind::Exp => "^",
            TokenKind::Lt => "<",
            TokenKind::Gt => ">",
            TokenKind::Eq => "=",
            TokenKind::Lte => "<=",
            TokenKind::Gte => ">=",
            TokenKind::Neq => "<>",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Comma => ",",
            TokenKind::Semicolon => ";",
            TokenKind::Colon => ":",
            TokenKind::Pipe => "|",
            TokenKind::Tilde => "~",
            TokenKind::Dot => ".",
            TokenKind::Bang => "!",
            TokenKind::DataEquals => ":=",
            TokenKind::Equiv => "<->",
            TokenKind::MapArrow => "->",
            TokenKind::Number => "Number",
            TokenKind::Symbol => "Symbol",
            TokenKind::Literal => "Literal",
            TokenKind::And => ":AND:",
            TokenKind::Or => ":OR:",
            TokenKind::Not => ":NOT:",
            TokenKind::Na => ":NA:",
            TokenKind::Macro => ":MACRO:",
            TokenKind::EndOfMacro => ":END OF MACRO:",
            TokenKind::Except => ":EXCEPT:",
            TokenKind::Interpolate => ":INTERPOLATE:",
            TokenKind::Raw => ":RAW:",
            TokenKind::HoldBackward => ":HOLD BACKWARD:",
            TokenKind::LookForward => ":LOOK FORWARD:",
            TokenKind::Implies => ":IMPLIES:",
            TokenKind::TestInput => ":TEST INPUT:",
            TokenKind::TheCondition => ":THE CONDITION:",
            TokenKind::EqEnd => "end of equation",
            TokenKind::GroupStar => "group marker",
            TokenKind::Question => "?",
            TokenKind::UnitsSymbol => "units symbol",
            TokenKind::Function => "function",
            TokenKind::WithLookup => "WITH LOOKUP",
            TokenKind::TabbedArray => "TABBED ARRAY",
        };
        f.write_str(name)
    }
}

fn token_kind(tok: &Token<'_>) -> TokenKind {
    match tok {
        Token::Plus => TokenKind::Plus,
        Token::Minus => TokenKind::Minus,
        Token::Mul => TokenKind::Mul,
        Token::Div => TokenKind::Div,
        Token::Exp => TokenKind::Exp,
        Token::Lt => TokenKind::Lt,
        Token::Gt => TokenKind::Gt,
        Token::Eq => TokenKind::Eq,
        Token::Lte => TokenKind::Lte,
        Token::Gte => TokenKind::Gte,
        Token::Neq => TokenKind::Neq,
        Token::LParen => TokenKind::LParen,
        Token::RParen => TokenKind::RParen,
        Token::LBracket => TokenKind::LBracket,
        Token::RBracket => TokenKind::RBracket,
        Token::Comma => TokenKind::Comma,
        Token::Semicolon => TokenKind::Semicolon,
        Token::Colon => TokenKind::Colon,
        Token::Pipe => TokenKind::Pipe,
        Token::Tilde => TokenKind::Tilde,
        Token::Dot => TokenKind::Dot,
        Token::Bang => TokenKind::Bang,
        Token::DataEquals => TokenKind::DataEquals,
        Token::Equiv => TokenKind::Equiv,
        Token::MapArrow => TokenKind::MapArrow,
        Token::Number(_) => TokenKind::Number,
        Token::Symbol(_) => TokenKind::Symbol,
        Token::Literal(_) => TokenKind::Literal,
        Token::And => TokenKind::And,
        Token::Or => TokenKind::Or,
        Token::Not => TokenKind::Not,
        Token::Na => TokenKind::Na,
        Token::Macro => TokenKind::Macro,
        Token::EndOfMacro => TokenKind::EndOfMacro,
        Token::Except => TokenKind::Except,
        Token::Interpolate => TokenKind::Interpolate,
        Token::Raw => TokenKind::Raw,
        Token::HoldBackward => TokenKind::HoldBackward,
        Token::LookForward => TokenKind::LookForward,
        Token::Implies => TokenKind::Implies,
        Token::TestInput => TokenKind::TestInput,
        Token::TheCondition => TokenKind::TheCondition,
        Token::EqEnd => TokenKind::EqEnd,
        Token::GroupStar(_) => TokenKind::GroupStar,
        Token::Question => TokenKind::Question,
        Token::UnitsSymbol(_) => TokenKind::UnitsSymbol,
        Token::Function(_) => TokenKind::Function,
        Token::WithLookup => TokenKind::WithLookup,
        Token::TabbedArray(_) => TokenKind::TabbedArray,
    }
}

// ============================================================================
// Parser struct
// ============================================================================

struct Parser<'input, 'tokens> {
    tokens: &'tokens [(usize, Token<'input>, usize)],
    pos: usize,
}

impl<'input, 'tokens> Parser<'input, 'tokens> {
    fn new(tokens: &'tokens [(usize, Token<'input>, usize)]) -> Self {
        Parser { tokens, pos: 0 }
    }

    /// Peek at the current token without consuming it.
    fn peek(&self) -> Option<&(usize, Token<'input>, usize)> {
        self.tokens.get(self.pos)
    }

    /// Peek at the kind of the current token.
    fn peek_kind(&self) -> Option<TokenKind> {
        self.peek().map(|(_, tok, _)| token_kind(tok))
    }

    /// Check if we're at end of token stream.
    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    /// Get the start position of the current token, or the end of the last token,
    /// or 0 if there are no tokens.
    fn start_pos(&self) -> usize {
        if let Some((start, _, _)) = self.peek() {
            *start
        } else if self.pos > 0 {
            self.tokens[self.pos - 1].2
        } else {
            0
        }
    }

    /// Get the end position of the last consumed token, or 0 if none consumed.
    fn end_pos(&self) -> usize {
        if self.pos > 0 {
            self.tokens[self.pos - 1].2
        } else {
            0
        }
    }

    /// Advance past the current token, returning its start and end positions.
    /// Use this when you only need position info, not the token data.
    fn advance_pos(&mut self) -> Option<(usize, usize)> {
        if self.pos < self.tokens.len() {
            let (l, _, r) = &self.tokens[self.pos];
            let result = (*l, *r);
            self.pos += 1;
            Some(result)
        } else {
            None
        }
    }

    /// Advance past the current token, returning a reference to the token triple.
    fn advance(&mut self) -> Option<&'tokens (usize, Token<'input>, usize)> {
        if self.pos < self.tokens.len() {
            let tok = &self.tokens[self.pos];
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    /// Consume the current token if it matches the expected kind.
    /// Returns position info on success, None on mismatch (does not advance).
    fn eat(&mut self, kind: TokenKind) -> Option<(usize, usize)> {
        if self.peek_kind() == Some(kind) {
            self.advance_pos()
        } else {
            None
        }
    }

    /// Consume the current token if it matches, returning a reference to the token.
    fn eat_ref(&mut self, kind: TokenKind) -> Option<&'tokens (usize, Token<'input>, usize)> {
        if self.peek_kind() == Some(kind) {
            self.advance()
        } else {
            None
        }
    }

    /// Expect and consume a specific token kind, returning position info.
    fn expect(&mut self, kind: TokenKind, what: &str) -> Result<(usize, usize), ParseError> {
        if let Some(pos) = self.eat(kind) {
            Ok(pos)
        } else if let Some((start, tok, end)) = self.peek() {
            Err(ParseError {
                start: *start,
                end: *end,
                message: format!("expected {}, found {}", what, token_kind(tok)),
            })
        } else {
            let pos = self.end_pos();
            Err(ParseError {
                start: pos,
                end: pos,
                message: format!("expected {}, found end of input", what),
            })
        }
    }

    /// Expect and consume a specific token kind, returning a reference to the token.
    fn expect_ref(
        &mut self,
        kind: TokenKind,
        what: &str,
    ) -> Result<&'tokens (usize, Token<'input>, usize), ParseError> {
        if let Some(tok) = self.eat_ref(kind) {
            Ok(tok)
        } else if let Some((start, tok, end)) = self.peek() {
            Err(ParseError {
                start: *start,
                end: *end,
                message: format!("expected {}, found {}", what, token_kind(tok)),
            })
        } else {
            let pos = self.end_pos();
            Err(ParseError {
                start: pos,
                end: pos,
                message: format!("expected {}, found end of input", what),
            })
        }
    }

    /// Expect a Symbol token and return its name.
    /// This is a convenience for the common pattern of expect(Symbol) + match extraction.
    fn expect_symbol(&mut self, what: &str) -> Result<Cow<'input, str>, ParseError> {
        let (_, tok, _) = self.expect_ref(TokenKind::Symbol, what)?;
        match tok {
            Token::Symbol(s) => Ok(s.clone()),
            _ => unreachable!(),
        }
    }

    /// Expect a Symbol token and return (start, name, end).
    fn expect_symbol_with_pos(
        &mut self,
        what: &str,
    ) -> Result<(usize, Cow<'input, str>, usize), ParseError> {
        let (l, tok, r) = self.expect_ref(TokenKind::Symbol, what)?;
        match tok {
            Token::Symbol(s) => Ok((*l, s.clone(), *r)),
            _ => unreachable!(),
        }
    }

    /// Create an unexpected-EOF error.
    fn eof_error(&self, what: &str) -> ParseError {
        let pos = self.end_pos();
        ParseError {
            start: pos,
            end: pos,
            message: format!("unexpected end of input while parsing {}", what),
        }
    }

    /// Create an unexpected-token error.
    fn unexpected_error(&self, what: &str) -> ParseError {
        if let Some((start, tok, end)) = self.peek() {
            ParseError {
                start: *start,
                end: *end,
                message: format!("unexpected {} while parsing {}", token_kind(tok), what),
            }
        } else {
            self.eof_error(what)
        }
    }

    // ========================================================================
    // Top-level entry point
    // ========================================================================

    /// Parse a full equation with units: eq ~ units ~ or eq ~ units |
    fn parse_full_eq_with_units(
        &mut self,
    ) -> Result<(Equation<'input>, Option<Units<'input>>, SectionEnd<'input>), ParseError> {
        // Check for special lead tokens
        match self.peek_kind() {
            Some(TokenKind::EqEnd) => {
                let (l, r) = self.advance_pos().unwrap();
                let loc = Loc::new(l, r);
                return Ok((
                    Equation::EmptyRhs(Lhs::empty(loc), loc),
                    None,
                    SectionEnd::EqEnd(loc),
                ));
            }
            Some(TokenKind::GroupStar) => {
                let (l, tok, r) = self.advance().unwrap();
                let loc = Loc::new(*l, *r);
                let name = match tok {
                    Token::GroupStar(name) => name.clone(),
                    _ => unreachable!(),
                };
                return Ok((
                    Equation::EmptyRhs(Lhs::empty(loc), loc),
                    None,
                    SectionEnd::GroupStar(name, loc),
                ));
            }
            Some(TokenKind::Macro) => {
                let (l, r) = self.advance_pos().unwrap();
                let loc = Loc::new(l, r);
                let name = self.expect_symbol("macro name")?;
                self.expect(TokenKind::LParen, "'('")?;
                let args = if self.peek_kind() == Some(TokenKind::RParen) {
                    vec![]
                } else {
                    self.parse_expr_list()?.into_exprs()
                };
                self.expect(TokenKind::RParen, "')'")?;
                return Ok((
                    Equation::EmptyRhs(Lhs::empty(loc), loc),
                    None,
                    SectionEnd::MacroStart(name, args, loc),
                ));
            }
            Some(TokenKind::EndOfMacro) => {
                let (l, r) = self.advance_pos().unwrap();
                let loc = Loc::new(l, r);
                return Ok((
                    Equation::EmptyRhs(Lhs::empty(loc), loc),
                    None,
                    SectionEnd::MacroEnd(loc),
                ));
            }
            _ => {}
        }

        // Parse the equation
        let eq = self.parse_eqn()?;

        // Expect tilde or pipe
        match self.peek_kind() {
            Some(TokenKind::Tilde) => {
                self.advance_pos(); // consume first tilde
                // Parse optional units
                let units = self.parse_optional_units()?;
                // Expect second tilde or pipe
                match self.peek_kind() {
                    Some(TokenKind::Tilde) => {
                        self.advance_pos();
                        Ok((eq, units, SectionEnd::Tilde))
                    }
                    Some(TokenKind::Pipe) => {
                        self.advance_pos();
                        Ok((eq, units, SectionEnd::Pipe))
                    }
                    _ => Err(self.unexpected_error("'~' or '|' after units")),
                }
            }
            Some(TokenKind::Pipe) => {
                self.advance_pos();
                Ok((eq, None, SectionEnd::Pipe))
            }
            _ => Err(self.unexpected_error("'~' or '|' after equation")),
        }
    }

    /// Parse optional units between first and second tilde.
    /// Returns None if the next token is ~ or |.
    fn parse_optional_units(&mut self) -> Result<Option<Units<'input>>, ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Tilde) | Some(TokenKind::Pipe) => Ok(None),
            _ => self.parse_units_range().map(Some),
        }
    }

    // ========================================================================
    // Equation parsing
    // ========================================================================

    /// Parse an equation (Eqn rule).
    /// Disambiguates between SubscriptDef, Equivalence, and LHS-based equations.
    fn parse_eqn(&mut self) -> Result<Equation<'input>, ParseError> {
        // All equation forms start with a Symbol token.
        // Save position to try disambiguation.
        if self.peek_kind() != Some(TokenKind::Symbol) {
            return Err(self.unexpected_error("equation (expected variable name)"));
        }

        let saved_pos = self.pos;
        let (sym_l, sym_tok, _sym_r) = self.advance().unwrap();
        let sym_name = match sym_tok {
            Token::Symbol(s) => s.clone(),
            _ => unreachable!(),
        };

        // Check what follows the first symbol
        match self.peek_kind() {
            Some(TokenKind::Colon) => {
                // SubscriptDef: Symbol : SubDef MapList
                self.advance_pos(); // consume ':'
                let mut def = self.parse_sub_def()?;
                let map = self.parse_map_list()?;
                def.mapping = map;
                Ok(Equation::SubscriptDef(sym_name, def))
            }
            Some(TokenKind::Equiv) => {
                // Equivalence: Symbol <-> Symbol
                let l = *sym_l;
                self.advance_pos(); // consume '<->'
                let (_, b_name, r) = self.expect_symbol_with_pos("symbol after '<->'")?;
                Ok(Equation::Equivalence(sym_name, b_name, Loc::new(l, r)))
            }
            _ => {
                // Restore position and parse as LHS-based equation
                self.pos = saved_pos;
                self.parse_lhs_equation()
            }
        }
    }

    /// Parse a LHS-based equation (regular, lookup, data, implicit, etc.)
    fn parse_lhs_equation(&mut self) -> Result<Equation<'input>, ParseError> {
        let lhs = self.parse_lhs()?;

        match self.peek_kind() {
            Some(TokenKind::Eq) => self.parse_eq_rhs(lhs),
            Some(TokenKind::LParen) => {
                // Lookup definition: lhs(table)
                self.parse_lookup_def(lhs)
            }
            Some(TokenKind::DataEquals) => {
                // Data equation: lhs := expr
                self.advance_pos(); // consume ':='
                let expr = self.parse_expr()?;
                Ok(Equation::Data(lhs, Some(expr)))
            }
            _ => {
                // Implicit (bare lhs)
                Ok(Equation::Implicit(lhs))
            }
        }
    }

    /// Parse the RHS after '='.
    /// Handles: Regular, EmptyRhs, WithLookup, TabbedArray
    fn parse_eq_rhs(&mut self, lhs: Lhs<'input>) -> Result<Equation<'input>, ParseError> {
        let (l, r) = self.expect(TokenKind::Eq, "'='")?;
        let eq_loc = Loc::new(l, r);

        // Check for TabbedArray
        if self.peek_kind() == Some(TokenKind::TabbedArray) {
            let (_, tok, _) = self.advance().unwrap();
            let values = match tok {
                Token::TabbedArray(values) => values.clone(),
                _ => unreachable!(),
            };
            return Ok(Equation::TabbedArray(lhs, values));
        }

        // Check for empty RHS (= followed by ~ or | or EOF)
        match self.peek_kind() {
            Some(TokenKind::Tilde) | Some(TokenKind::Pipe) | None => {
                return Ok(Equation::EmptyRhs(lhs, eq_loc));
            }
            _ => {}
        }

        // Check for WITH LOOKUP
        if self.peek_kind() == Some(TokenKind::WithLookup) {
            self.advance_pos(); // consume WITH LOOKUP
            self.expect(TokenKind::LParen, "'(' after WITH LOOKUP")?;
            let expr = self.parse_expr()?;
            self.expect(TokenKind::Comma, "',' in WITH LOOKUP")?;
            self.expect(TokenKind::LParen, "'(' for table in WITH LOOKUP")?;
            let table = self.parse_table_vals()?;
            self.expect(TokenKind::RParen, "')' for table in WITH LOOKUP")?;
            self.expect(TokenKind::RParen, "')' for WITH LOOKUP")?;
            return Ok(Equation::WithLookup(lhs, Box::new(expr), table));
        }

        // Regular equation: parse expression list
        let exprs = self.parse_expr_list()?;
        make_equation(lhs, exprs)
    }

    /// Parse a lookup definition: lhs(table)
    fn parse_lookup_def(&mut self, lhs: Lhs<'input>) -> Result<Equation<'input>, ParseError> {
        // Use LHS start for error spans, matching LALRPOP's @L which covered
        // from the start of the entire production (including the LHS Symbol).
        let l = lhs.loc.start as usize;
        self.expect(TokenKind::LParen, "'(' for lookup")?;

        // Disambiguate table format
        match self.peek_kind() {
            Some(TokenKind::LParen) => {
                // Pairs format: (x,y), (x,y), ...
                let table = self.parse_table_pairs()?;
                self.expect(TokenKind::RParen, "')'")?;
                Ok(Equation::Lookup(lhs, table))
            }
            Some(TokenKind::LBracket) => {
                // Range prefix: [(xmin,ymin)-(xmax,ymax)], ...
                let (x1, y1, x2, y2) = self.parse_range_prefix()?;

                // After range prefix, check what follows
                match self.peek_kind() {
                    Some(TokenKind::LParen) => {
                        // Pairs after range
                        let mut table = self.parse_table_pairs()?;
                        table.set_range(x1, y1, x2, y2);
                        self.expect(TokenKind::RParen, "')'")?;
                        Ok(Equation::Lookup(lhs, table))
                    }
                    _ => {
                        // Legacy XY format after range
                        let mut table = self.parse_xy_table_vec()?;
                        table.set_range(x1, y1, x2, y2);
                        let r = self.start_pos();
                        table.transform_legacy().map_err(|msg| ParseError {
                            start: l,
                            end: r,
                            message: msg.to_string(),
                        })?;
                        self.expect(TokenKind::RParen, "')'")?;
                        Ok(Equation::Lookup(lhs, table))
                    }
                }
            }
            _ => {
                // Legacy XY format (starts with number or sign)
                let mut table = self.parse_xy_table_vec()?;
                let r = self.start_pos();
                table.transform_legacy().map_err(|msg| ParseError {
                    start: l,
                    end: r,
                    message: msg.to_string(),
                })?;
                self.expect(TokenKind::RParen, "')'")?;
                Ok(Equation::Lookup(lhs, table))
            }
        }
    }

    // ========================================================================
    // LHS parsing
    // ========================================================================

    /// Parse a left-hand side: Var [ExceptList | InterpMode]
    fn parse_lhs(&mut self) -> Result<Lhs<'input>, ParseError> {
        let l = self.start_pos();
        let (name, subscripts) = self.parse_var()?;

        // Check for except or interp mode
        match self.peek_kind() {
            Some(TokenKind::Except) => {
                let except = self.parse_except_list()?;
                let r = self.end_pos();
                Ok(Lhs {
                    name,
                    subscripts,
                    except: Some(except),
                    interp_mode: None,
                    loc: Loc::new(l, r),
                })
            }
            Some(TokenKind::Interpolate)
            | Some(TokenKind::Raw)
            | Some(TokenKind::HoldBackward)
            | Some(TokenKind::LookForward) => {
                let interp = self.parse_interp_mode()?;
                let r = self.end_pos();
                Ok(Lhs {
                    name,
                    subscripts,
                    except: None,
                    interp_mode: Some(interp),
                    loc: Loc::new(l, r),
                })
            }
            _ => {
                let r = self.end_pos();
                Ok(Lhs {
                    name,
                    subscripts,
                    except: None,
                    interp_mode: None,
                    loc: Loc::new(l, r),
                })
            }
        }
    }

    /// Parse a variable: Symbol [SubList]
    fn parse_var(&mut self) -> Result<(Cow<'input, str>, Vec<Subscript<'input>>), ParseError> {
        let name = self.expect_symbol("variable name")?;

        let subs = if self.peek_kind() == Some(TokenKind::LBracket) {
            self.parse_sub_list()?
        } else {
            vec![]
        };

        Ok((name, subs))
    }

    /// Parse a subscript list: [sym, sym, ...]
    fn parse_sub_list(&mut self) -> Result<Vec<Subscript<'input>>, ParseError> {
        self.expect(TokenKind::LBracket, "'['")?;
        let list = self.parse_sym_list()?;
        self.expect(TokenKind::RBracket, "']'")?;
        Ok(list)
    }

    /// Parse a comma-separated symbol list with optional bang: sym!, sym, ...
    fn parse_sym_list(&mut self) -> Result<Vec<Subscript<'input>>, ParseError> {
        let mut list = Vec::new();

        let (l, name, r) = self.expect_symbol_with_pos("symbol in subscript list")?;

        if self.eat(TokenKind::Bang).is_some() {
            let r2 = self.end_pos();
            list.push(Subscript::BangElement(name, Loc::new(l, r2)));
        } else {
            list.push(Subscript::Element(name, Loc::new(l, r)));
        }

        while self.eat(TokenKind::Comma).is_some() {
            let (l, name, r) = self.expect_symbol_with_pos("symbol in subscript list")?;
            if self.eat(TokenKind::Bang).is_some() {
                let r2 = self.end_pos();
                list.push(Subscript::BangElement(name, Loc::new(l, r2)));
            } else {
                list.push(Subscript::Element(name, Loc::new(l, r)));
            }
        }

        Ok(list)
    }

    /// Parse a subscript definition: sym | (start - end), more...
    fn parse_sub_def(&mut self) -> Result<SubscriptDef<'input>, ParseError> {
        let mut elements = Vec::new();
        let first_l = self.start_pos();

        // Parse first element
        let elem = self.parse_sub_def_element()?;
        elements.push(elem);

        // Parse additional elements separated by commas
        while self.eat(TokenKind::Comma).is_some() {
            let elem = self.parse_sub_def_element()?;
            elements.push(elem);
        }

        let last_r = self.end_pos();
        Ok(SubscriptDef {
            elements,
            mapping: None,
            loc: Loc::new(first_l, last_r),
        })
    }

    /// Parse a single subscript definition element: Symbol or (Symbol - Symbol)
    fn parse_sub_def_element(&mut self) -> Result<SubscriptElement<'input>, ParseError> {
        if self.peek_kind() == Some(TokenKind::LParen) {
            // Range element: (start - end)
            let (l, _) = self.advance_pos().unwrap(); // consume '('
            let start_name = self.expect_symbol("range start")?;
            self.expect(TokenKind::Minus, "'-' in range")?;
            let end_name = self.expect_symbol("range end")?;
            let (_, r) = self.expect(TokenKind::RParen, "')'")?;
            Ok(SubscriptElement::Range(
                start_name,
                end_name,
                Loc::new(l, r),
            ))
        } else {
            // Simple element
            let (l, name, r) = self.expect_symbol_with_pos("subscript element")?;
            Ok(SubscriptElement::Element(name, Loc::new(l, r)))
        }
    }

    /// Parse an exception list: :EXCEPT: [SubList] (, [SubList])*
    fn parse_except_list(&mut self) -> Result<ExceptList<'input>, ParseError> {
        let (l, _) = self.expect(TokenKind::Except, ":EXCEPT:")?;
        let first_sub = self.parse_sub_list()?;
        let mut subscripts = vec![first_sub];

        while self.eat(TokenKind::Comma).is_some() {
            let sub = self.parse_sub_list()?;
            subscripts.push(sub);
        }

        let r = self.end_pos();
        Ok(ExceptList {
            subscripts,
            loc: Loc::new(l, r),
        })
    }

    /// Parse an interpolation mode keyword.
    fn parse_interp_mode(&mut self) -> Result<InterpMode, ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Interpolate) => {
                self.advance_pos();
                Ok(InterpMode::Interpolate)
            }
            Some(TokenKind::Raw) => {
                self.advance_pos();
                Ok(InterpMode::Raw)
            }
            Some(TokenKind::HoldBackward) => {
                self.advance_pos();
                Ok(InterpMode::HoldBackward)
            }
            Some(TokenKind::LookForward) => {
                self.advance_pos();
                Ok(InterpMode::LookForward)
            }
            _ => Err(self.unexpected_error("interpolation mode")),
        }
    }

    /// Parse an optional mapping list: -> MapSymList | empty
    fn parse_map_list(&mut self) -> Result<Option<SubscriptMapping<'input>>, ParseError> {
        if self.eat(TokenKind::MapArrow).is_some() {
            let map = self.parse_map_sym_list()?;
            Ok(Some(map))
        } else {
            Ok(None)
        }
    }

    /// Parse a mapping symbol list: sym | (dim: symlist), more...
    fn parse_map_sym_list(&mut self) -> Result<SubscriptMapping<'input>, ParseError> {
        let mut entries = Vec::new();
        let first_l = self.start_pos();

        let entry = self.parse_map_entry()?;
        entries.push(entry);

        while self.eat(TokenKind::Comma).is_some() {
            let entry = self.parse_map_entry()?;
            entries.push(entry);
        }

        let last_r = self.end_pos();
        Ok(SubscriptMapping {
            entries,
            loc: Loc::new(first_l, last_r),
        })
    }

    /// Parse a single mapping entry: Symbol or (Symbol: SymList)
    fn parse_map_entry(&mut self) -> Result<MappingEntry<'input>, ParseError> {
        if self.peek_kind() == Some(TokenKind::LParen) {
            // Dimension mapping: (DimB: elem1, elem2)
            let (l, _) = self.advance_pos().unwrap(); // consume '('
            let dim = self.expect_symbol("dimension name in mapping")?;
            self.expect(TokenKind::Colon, "':' in dimension mapping")?;
            let list = self.parse_sym_list()?;
            let (_, r) = self.expect(TokenKind::RParen, "')'")?;
            Ok(MappingEntry::DimensionMapping {
                dimension: dim,
                elements: list,
                loc: Loc::new(l, r),
            })
        } else {
            // Simple name
            let (l, name, r) = self.expect_symbol_with_pos("symbol in mapping")?;
            Ok(MappingEntry::Name(name, Loc::new(l, r)))
        }
    }

    // ========================================================================
    // Expression parsing (8 precedence levels)
    // ========================================================================

    /// Parse an expression (entry point, same as AddSub).
    fn parse_expr(&mut self) -> Result<Expr<'input>, ParseError> {
        self.parse_add_sub()
    }

    /// Level 1 (lowest): AddSub -- +, - (left-assoc)
    fn parse_add_sub(&mut self) -> Result<Expr<'input>, ParseError> {
        let l = self.start_pos();
        let mut left = self.parse_logic_or()?;

        loop {
            match self.peek_kind() {
                Some(TokenKind::Plus) => {
                    self.advance_pos();
                    let right = self.parse_logic_or()?;
                    let r = self.end_pos();
                    left = Expr::Op2(
                        BinaryOp::Add,
                        Box::new(left),
                        Box::new(right),
                        Loc::new(l, r),
                    );
                }
                Some(TokenKind::Minus) => {
                    self.advance_pos();
                    let right = self.parse_logic_or()?;
                    let r = self.end_pos();
                    left = Expr::Op2(
                        BinaryOp::Sub,
                        Box::new(left),
                        Box::new(right),
                        Loc::new(l, r),
                    );
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// Level 2: LogicOr -- :OR: (left-assoc)
    fn parse_logic_or(&mut self) -> Result<Expr<'input>, ParseError> {
        let l = self.start_pos();
        let mut left = self.parse_cmp()?;

        while self.peek_kind() == Some(TokenKind::Or) {
            self.advance_pos();
            let right = self.parse_cmp()?;
            let r = self.end_pos();
            left = Expr::Op2(
                BinaryOp::Or,
                Box::new(left),
                Box::new(right),
                Loc::new(l, r),
            );
        }

        Ok(left)
    }

    /// Level 3: Cmp -- =, <, >, <=, >=, <> (left-assoc)
    fn parse_cmp(&mut self) -> Result<Expr<'input>, ParseError> {
        let l = self.start_pos();
        let mut left = self.parse_logic_and()?;

        loop {
            let op = match self.peek_kind() {
                Some(TokenKind::Eq) => BinaryOp::Eq,
                Some(TokenKind::Lt) => BinaryOp::Lt,
                Some(TokenKind::Gt) => BinaryOp::Gt,
                Some(TokenKind::Lte) => BinaryOp::Lte,
                Some(TokenKind::Gte) => BinaryOp::Gte,
                Some(TokenKind::Neq) => BinaryOp::Neq,
                _ => break,
            };
            self.advance_pos();
            let right = self.parse_logic_and()?;
            let r = self.end_pos();
            left = Expr::Op2(op, Box::new(left), Box::new(right), Loc::new(l, r));
        }

        Ok(left)
    }

    /// Level 4: LogicAnd -- :AND: (left-assoc)
    fn parse_logic_and(&mut self) -> Result<Expr<'input>, ParseError> {
        let l = self.start_pos();
        let mut left = self.parse_mul_div()?;

        while self.peek_kind() == Some(TokenKind::And) {
            self.advance_pos();
            let right = self.parse_mul_div()?;
            let r = self.end_pos();
            left = Expr::Op2(
                BinaryOp::And,
                Box::new(left),
                Box::new(right),
                Loc::new(l, r),
            );
        }

        Ok(left)
    }

    /// Level 5: MulDiv -- *, / (left-assoc)
    fn parse_mul_div(&mut self) -> Result<Expr<'input>, ParseError> {
        let l = self.start_pos();
        let mut left = self.parse_unary()?;

        loop {
            match self.peek_kind() {
                Some(TokenKind::Mul) => {
                    self.advance_pos();
                    let right = self.parse_unary()?;
                    let r = self.end_pos();
                    left = Expr::Op2(
                        BinaryOp::Mul,
                        Box::new(left),
                        Box::new(right),
                        Loc::new(l, r),
                    );
                }
                Some(TokenKind::Div) => {
                    self.advance_pos();
                    let right = self.parse_unary()?;
                    let r = self.end_pos();
                    left = Expr::Op2(
                        BinaryOp::Div,
                        Box::new(left),
                        Box::new(right),
                        Loc::new(l, r),
                    );
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// Level 6: Unary -- :NOT:, unary +, unary - (prefix, recursive)
    fn parse_unary(&mut self) -> Result<Expr<'input>, ParseError> {
        let l = self.start_pos();
        match self.peek_kind() {
            Some(TokenKind::Not) => {
                self.advance_pos();
                let inner = self.parse_unary()?;
                let r = self.end_pos();
                Ok(Expr::Op1(UnaryOp::Not, Box::new(inner), Loc::new(l, r)))
            }
            Some(TokenKind::Minus) => {
                self.advance_pos();
                let inner = self.parse_unary()?;
                let r = self.end_pos();
                Ok(Expr::Op1(
                    UnaryOp::Negative,
                    Box::new(inner),
                    Loc::new(l, r),
                ))
            }
            Some(TokenKind::Plus) => {
                self.advance_pos();
                let inner = self.parse_unary()?;
                let r = self.end_pos();
                Ok(Expr::Op1(
                    UnaryOp::Positive,
                    Box::new(inner),
                    Loc::new(l, r),
                ))
            }
            _ => self.parse_power(),
        }
    }

    /// Level 7: Power -- ^ (right-associative)
    fn parse_power(&mut self) -> Result<Expr<'input>, ParseError> {
        let l = self.start_pos();
        let base = self.parse_atom()?;

        if self.peek_kind() == Some(TokenKind::Exp) {
            self.advance_pos();
            // Right-associative: exponent calls parse_unary (which can recurse to power)
            let exp = self.parse_unary()?;
            let r = self.end_pos();
            Ok(Expr::Op2(
                BinaryOp::Exp,
                Box::new(base),
                Box::new(exp),
                Loc::new(l, r),
            ))
        } else {
            Ok(base)
        }
    }

    /// Level 8 (highest): Atom -- literals, vars, calls, parens
    fn parse_atom(&mut self) -> Result<Expr<'input>, ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Number) => {
                let &(l, ref tok, r) = self.advance().unwrap();
                let s = match tok {
                    Token::Number(s) => s,
                    _ => unreachable!(),
                };
                let val = parse_number(s, l, r)?;
                Ok(Expr::Const(val, Loc::new(l, r)))
            }
            Some(TokenKind::Na) => {
                let (l, r) = self.advance_pos().unwrap();
                Ok(Expr::Na(Loc::new(l, r)))
            }
            Some(TokenKind::Literal) => {
                let &(l, ref tok, r) = self.advance().unwrap();
                let lit = match tok {
                    Token::Literal(s) => s.clone(),
                    _ => unreachable!(),
                };
                Ok(Expr::Literal(lit, Loc::new(l, r)))
            }
            Some(TokenKind::LParen) => {
                let (l, _) = self.advance_pos().unwrap();
                let inner = self.parse_expr()?;
                let (_, r) = self.expect(TokenKind::RParen, "')'")?;
                Ok(Expr::Paren(Box::new(inner), Loc::new(l, r)))
            }
            Some(TokenKind::Symbol) => {
                let l = self.start_pos();
                let (name, subscripts) = self.parse_var()?;

                // Check for function call: var(args)
                // Symbol calls require at least one argument (ExprList),
                // unlike builtin Function calls which allow empty parens.
                if self.peek_kind() == Some(TokenKind::LParen) {
                    self.advance_pos(); // consume '('
                    if self.peek_kind() == Some(TokenKind::RParen) {
                        let (start, end) = self.advance_pos().unwrap();
                        return Err(ParseError {
                            start,
                            end,
                            message: "symbol call requires at least one argument".to_string(),
                        });
                    }
                    let args = self.parse_expr_list()?.into_exprs();
                    let (_, r) = self.expect(TokenKind::RParen, "')'")?;
                    Ok(Expr::App(
                        name,
                        subscripts,
                        args,
                        CallKind::Symbol,
                        Loc::new(l, r),
                    ))
                } else {
                    let r = self.end_pos();
                    Ok(Expr::Var(name, subscripts, Loc::new(l, r)))
                }
            }
            Some(TokenKind::Function) => {
                let &(l, ref tok, _) = self.advance().unwrap();
                let name = match tok {
                    Token::Function(s) => s.clone(),
                    _ => unreachable!(),
                };
                self.expect(TokenKind::LParen, "'(' after function name")?;

                // Empty args: FUNC()
                if self.peek_kind() == Some(TokenKind::RParen) {
                    let (_, r) = self.advance_pos().unwrap();
                    return Ok(Expr::App(
                        name,
                        vec![],
                        vec![],
                        CallKind::Builtin,
                        Loc::new(l, r),
                    ));
                }

                // Parse arguments with trailing comma support.
                // We parse the first expression, then loop on comma-separated args,
                // checking for `,)` (trailing comma) at each step.
                let mut exprs = vec![self.parse_expr()?];

                loop {
                    match self.peek_kind() {
                        Some(TokenKind::Comma) => {
                            self.advance_pos(); // consume comma
                            // Check for trailing comma: `,)`
                            if self.peek_kind() == Some(TokenKind::RParen) {
                                let r_pos = self.start_pos();
                                exprs.push(Expr::Literal(
                                    Cow::Borrowed("?"),
                                    Loc::new(r_pos.saturating_sub(1), r_pos),
                                ));
                                break;
                            }
                            exprs.push(self.parse_expr()?);
                        }
                        Some(TokenKind::Semicolon) => {
                            self.advance_pos(); // consume semicolon
                            // Trailing semicolon check
                            if self.peek_kind() == Some(TokenKind::RParen) {
                                break;
                            }
                            exprs.push(self.parse_expr()?);
                        }
                        _ => break,
                    }
                }

                let (_, r) = self.expect(TokenKind::RParen, "')'")?;
                Ok(Expr::App(
                    name,
                    vec![],
                    exprs,
                    CallKind::Builtin,
                    Loc::new(l, r),
                ))
            }
            _ => Err(self.unexpected_error("expression")),
        }
    }

    /// Parse a comma/semicolon separated expression list.
    fn parse_expr_list(&mut self) -> Result<ExprListResult<'input>, ParseError> {
        let first = self.parse_expr()?;
        let mut result = ExprListResult::Single(first);

        loop {
            match self.peek_kind() {
                Some(TokenKind::Comma) => {
                    self.advance_pos();
                    let next = self.parse_expr()?;
                    result = result.append(next);
                }
                Some(TokenKind::Semicolon) => {
                    self.advance_pos();
                    // Trailing semicolon: check if next token is an expression start
                    match self.peek_kind() {
                        Some(TokenKind::Number)
                        | Some(TokenKind::Symbol)
                        | Some(TokenKind::Function)
                        | Some(TokenKind::LParen)
                        | Some(TokenKind::Minus)
                        | Some(TokenKind::Plus)
                        | Some(TokenKind::Not)
                        | Some(TokenKind::Na)
                        | Some(TokenKind::Literal) => {
                            let next = self.parse_expr()?;
                            result = result.append(next);
                        }
                        _ => {
                            // Trailing semicolon with no expression after -- just stop
                            break;
                        }
                    }
                }
                _ => break,
            }
        }

        Ok(result)
    }

    // ========================================================================
    // Lookup table parsing
    // ========================================================================

    /// Parse table values: pairs format with optional range prefix.
    fn parse_table_vals(&mut self) -> Result<LookupTable, ParseError> {
        if self.peek_kind() == Some(TokenKind::LBracket) {
            // Range prefix
            let (x1, y1, x2, y2) = self.parse_range_prefix()?;
            let mut table = self.parse_table_pairs()?;
            table.set_range(x1, y1, x2, y2);
            Ok(table)
        } else {
            self.parse_table_pairs()
        }
    }

    /// Parse range prefix: [(x1,y1)-(x2,y2)] or [(x1,y1)-(x2,y2), inner_pairs]
    /// Returns (x1, y1, x2, y2) after consuming trailing comma.
    fn parse_range_prefix(&mut self) -> Result<(f64, f64, f64, f64), ParseError> {
        self.expect(TokenKind::LBracket, "'['")?;
        self.expect(TokenKind::LParen, "'('")?;
        let x1 = self.parse_signed_number()?;
        self.expect(TokenKind::Comma, "','")?;
        let y1 = self.parse_signed_number()?;
        self.expect(TokenKind::RParen, "')'")?;
        self.expect(TokenKind::Minus, "'-'")?;
        self.expect(TokenKind::LParen, "'('")?;
        let x2 = self.parse_signed_number()?;
        self.expect(TokenKind::Comma, "','")?;
        let y2 = self.parse_signed_number()?;
        self.expect(TokenKind::RParen, "')'")?;

        // Check for embedded pairs variant: [(x1,y1)-(x2,y2), pairs], more
        if self.peek_kind() == Some(TokenKind::Comma) {
            // Could be embedded pairs (skipped) or end of range prefix
            // If after comma we see '(' it's embedded pairs
            let saved = self.pos;
            self.advance_pos(); // consume comma

            if self.peek_kind() == Some(TokenKind::LParen) {
                // Embedded pairs: parse and validate as TablePairs, then discard.
                // The LALRPOP grammar parsed these as TablePairs so malformed
                // inner pairs were rejected; we preserve that validation.
                let _inner = self.parse_table_pairs()?;
                self.expect(TokenKind::RBracket, "']'")?;
                self.expect(TokenKind::Comma, "',' after range prefix")?;
            } else {
                // Not embedded pairs -- this was the separator between range and data.
                // The bracket must close first. Restore and handle.
                self.pos = saved;
                self.expect(TokenKind::RBracket, "']'")?;
                self.expect(TokenKind::Comma, "',' after range prefix")?;
            }
        } else {
            self.expect(TokenKind::RBracket, "']'")?;
            self.expect(TokenKind::Comma, "',' after range prefix")?;
        }

        Ok((x1, y1, x2, y2))
    }

    /// Parse table pairs: (x,y), (x,y), ...
    fn parse_table_pairs(&mut self) -> Result<LookupTable, ParseError> {
        let l = self.start_pos();
        self.expect(TokenKind::LParen, "'(' for pair")?;
        let x = self.parse_signed_number()?;
        self.expect(TokenKind::Comma, "',' in pair")?;
        let y = self.parse_signed_number()?;
        self.expect(TokenKind::RParen, "')' for pair")?;
        let r = self.end_pos();

        let mut table = LookupTable::new(Loc::new(l, r));
        table.add_pair(x, y);

        while self.peek_kind() == Some(TokenKind::Comma) {
            // Peek ahead to distinguish pair comma from other commas
            // A pair always follows: , (x, y)
            if self.pos + 1 < self.tokens.len()
                && token_kind(&self.tokens[self.pos + 1].1) == TokenKind::LParen
            {
                self.advance_pos(); // consume comma
                self.expect(TokenKind::LParen, "'(' for pair")?;
                let x = self.parse_signed_number()?;
                self.expect(TokenKind::Comma, "',' in pair")?;
                let y = self.parse_signed_number()?;
                let (_, r) = self.expect(TokenKind::RParen, "')' for pair")?;
                table.add_pair(x, y);
                table.loc = Loc::merge(table.loc, Loc::new(table.loc.end as usize, r));
            } else {
                break;
            }
        }

        Ok(table)
    }

    /// Parse an XY table vector: n, n, n, ... (flat numbers for legacy format)
    fn parse_xy_table_vec(&mut self) -> Result<LookupTable, ParseError> {
        let l = self.start_pos();
        let n = self.parse_signed_number()?;
        let r = self.end_pos();

        let mut table = LookupTable::new_legacy(Loc::new(l, r));
        table.add_raw(n);

        while self.peek_kind() == Some(TokenKind::Comma) {
            // Check if next after comma is a number (or sign+number)
            let after_comma = if self.pos + 1 < self.tokens.len() {
                Some(token_kind(&self.tokens[self.pos + 1].1))
            } else {
                None
            };

            match after_comma {
                Some(TokenKind::Number) | Some(TokenKind::Minus) | Some(TokenKind::Plus) => {
                    self.advance_pos(); // consume comma
                    let n = self.parse_signed_number()?;
                    let r = self.end_pos();
                    table.add_raw(n);
                    table.loc = Loc::merge(table.loc, Loc::new(table.loc.end as usize, r));
                }
                _ => break,
            }
        }

        Ok(table)
    }

    /// Parse a number with optional leading sign (for lookup tables, NOT full expressions).
    fn parse_signed_number(&mut self) -> Result<f64, ParseError> {
        match self.peek_kind() {
            Some(TokenKind::Minus) => {
                self.advance_pos();
                let &(l, ref tok, r) = self.expect_ref(TokenKind::Number, "number after '-'")?;
                let s = match tok {
                    Token::Number(s) => s,
                    _ => unreachable!(),
                };
                let val = parse_number(s, l, r)?;
                Ok(-val)
            }
            Some(TokenKind::Plus) => {
                self.advance_pos();
                let &(l, ref tok, r) = self.expect_ref(TokenKind::Number, "number after '+'")?;
                let s = match tok {
                    Token::Number(s) => s,
                    _ => unreachable!(),
                };
                parse_number(s, l, r)
            }
            Some(TokenKind::Number) => {
                let &(l, ref tok, r) = self.advance().unwrap();
                let s = match tok {
                    Token::Number(s) => s,
                    _ => unreachable!(),
                };
                parse_number(s, l, r)
            }
            _ => Err(self.unexpected_error("number")),
        }
    }

    // ========================================================================
    // Units parsing
    // ========================================================================

    /// Parse units with optional range.
    fn parse_units_range(&mut self) -> Result<Units<'input>, ParseError> {
        let l = self.start_pos();

        // Check if it starts with '[' (range-only, no units expr)
        if self.peek_kind() == Some(TokenKind::LBracket) {
            let range = self.parse_unit_range_bracket()?;
            let r = self.end_pos();
            return Ok(Units {
                expr: None,
                range: Some(range),
                loc: Loc::new(l, r),
            });
        }

        // Parse unit expression
        let expr = self.parse_unit_expr()?;

        // Check for optional range
        if self.peek_kind() == Some(TokenKind::LBracket) {
            let range = self.parse_unit_range_bracket()?;
            let r = self.end_pos();
            Ok(Units {
                expr: Some(expr),
                range: Some(range),
                loc: Loc::new(l, r),
            })
        } else {
            let r = self.end_pos();
            Ok(Units {
                expr: Some(expr),
                range: None,
                loc: Loc::new(l, r),
            })
        }
    }

    /// Parse a unit range bracket: [min, max] or [min, max, step]
    fn parse_unit_range_bracket(&mut self) -> Result<UnitRange, ParseError> {
        self.expect(TokenKind::LBracket, "'['")?;
        let min = self.parse_urange_num()?;
        self.expect(TokenKind::Comma, "',' in range")?;
        let max = self.parse_urange_num()?;

        let step = if self.eat(TokenKind::Comma).is_some() {
            self.parse_urange_num()?
        } else {
            None
        };

        self.expect(TokenKind::RBracket, "']'")?;

        Ok(UnitRange { min, max, step })
    }

    /// Parse a unit range number: number or ?
    fn parse_urange_num(&mut self) -> Result<Option<f64>, ParseError> {
        if self.eat(TokenKind::Question).is_some() {
            Ok(None)
        } else {
            let val = self.parse_signed_number()?;
            Ok(Some(val))
        }
    }

    /// Parse a unit expression (left-associative * and /).
    fn parse_unit_expr(&mut self) -> Result<UnitExpr<'input>, ParseError> {
        let l = self.start_pos();
        let mut left = self.parse_unit_term()?;

        loop {
            match self.peek_kind() {
                Some(TokenKind::Div) => {
                    self.advance_pos();
                    let right = self.parse_unit_term()?;
                    let r = self.end_pos();
                    left = UnitExpr::Div(Box::new(left), Box::new(right), Loc::new(l, r));
                }
                Some(TokenKind::Mul) => {
                    self.advance_pos();
                    let right = self.parse_unit_term()?;
                    let r = self.end_pos();
                    left = UnitExpr::Mul(Box::new(left), Box::new(right), Loc::new(l, r));
                }
                _ => break,
            }
        }

        Ok(left)
    }

    /// Parse a unit term: UnitsSymbol or (UnitExpr).
    fn parse_unit_term(&mut self) -> Result<UnitExpr<'input>, ParseError> {
        if self.peek_kind() == Some(TokenKind::LParen) {
            self.advance_pos(); // consume '('
            let inner = self.parse_unit_expr()?;
            self.expect(TokenKind::RParen, "')'")?;
            Ok(inner)
        } else {
            let &(l, ref tok, r) = self.expect_ref(TokenKind::UnitsSymbol, "unit name")?;
            let name = match tok {
                Token::UnitsSymbol(s) => s.clone(),
                _ => unreachable!(),
            };
            Ok(UnitExpr::Unit(name, Loc::new(l, r)))
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Parse a token stream into an equation with units and section end.
///
/// This is the public entry point used by EquationReader.
/// Takes a borrowed slice to avoid allocating a copy of the token vector.
pub fn parse<'input>(
    tokens: &[(usize, Token<'input>, usize)],
) -> Result<(Equation<'input>, Option<Units<'input>>, SectionEnd<'input>), ParseError> {
    let mut parser = Parser::new(tokens);
    let result = parser.parse_full_eq_with_units()?;

    // Full consumption check
    if !parser.at_end() {
        let (start, tok, end) = &parser.tokens[parser.pos];
        return Err(ParseError {
            start: *start,
            end: *end,
            message: format!("unexpected trailing token: {}", token_kind(tok)),
        });
    }

    Ok(result)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::ast::BinaryOp;
    use std::borrow::Cow;

    /// Helper to collect tokens from normalizer into a Vec suitable for parse().
    fn collect_tokens<'a>(
        normalizer: impl Iterator<Item = Result<(usize, Token<'a>, usize), NormalizerError>>,
    ) -> Vec<(usize, Token<'a>, usize)> {
        normalizer.map(|r| r.unwrap()).collect()
    }

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
    // parse_number tests (from parser_helpers.rs)
    // ========================================================================

    #[test]
    fn test_parse_number_integer() {
        assert_eq!(parse_number("42", 0, 2).unwrap(), 42.0);
    }

    #[test]
    fn test_parse_number_float() {
        assert_eq!(parse_number("2.5", 0, 3).unwrap(), 2.5);
    }

    #[test]
    fn test_parse_number_scientific() {
        assert_eq!(parse_number("1e6", 0, 3).unwrap(), 1_000_000.0);
        assert_eq!(parse_number("1.5e-3", 0, 6).unwrap(), 0.0015);
    }

    #[test]
    fn test_parse_number_invalid() {
        let err = parse_number("not_a_number", 10, 22).unwrap_err();
        assert_eq!(err.start, 10);
        assert_eq!(err.end, 22);
        assert!(err.message.contains("invalid number"));
    }

    // ========================================================================
    // extract_number tests (from parser_helpers.rs)
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
        let inner = Expr::Const(7.0, loc());
        let expr = Expr::Op1(UnaryOp::Positive, Box::new(inner), loc());
        assert_eq!(extract_number(&expr), None);
    }

    #[test]
    fn test_extract_number_nested_unary_returns_none() {
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
    // make_equation tests (from parser_helpers.rs)
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
            Expr::Var(Cow::Borrowed("a"), vec![], loc()),
            Expr::Const(3.0, loc()),
        ];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("item 1"));
        assert!(err.message.contains("not a numeric literal"));
    }

    #[test]
    fn test_make_equation_mixed_list_first_item_non_numeric() {
        let lhs = make_lhs("x");
        let items = vec![
            Expr::Var(Cow::Borrowed("a"), vec![], loc()),
            Expr::Const(2.0, loc()),
        ];
        let result = make_equation(lhs, ExprListResult::Multiple(items));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("item 0"));
    }

    // ========================================================================
    // :NA: handling tests (from parser_helpers.rs)
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

    // ========================================================================
    // Error span tests
    // ========================================================================

    #[test]
    fn test_error_span_unexpected_token() {
        // Feed tokens that produce a parse error mid-stream
        let tokens = vec![
            (0, Token::Symbol(Cow::Borrowed("x")), 1),
            (2, Token::Eq, 3),
            // Bad: another Eq where an expression is expected
            (4, Token::Eq, 5),
            (6, Token::Tilde, 7),
            (8, Token::Tilde, 9),
        ];
        let err = parse(&tokens).unwrap_err();
        assert_eq!(err.start, 4);
        assert_eq!(err.end, 5);
    }

    #[test]
    fn test_error_span_eof_with_tokens() {
        // Truncated: Symbol = with no RHS and no tilde
        let tokens = vec![(0, Token::Symbol(Cow::Borrowed("x")), 1), (2, Token::Eq, 3)];
        let err = parse(&tokens).unwrap_err();
        // EOF error should use end position of last consumed token
        assert!(err.start >= 3 || err.start == 0);
    }

    #[test]
    fn test_error_span_eof_empty() {
        let tokens: Vec<(usize, Token, usize)> = vec![];
        let err = parse(&tokens).unwrap_err();
        assert_eq!(err.start, 0);
        assert_eq!(err.end, 0);
    }

    // ========================================================================
    // Parity test: parse through normalizer and compare
    // ========================================================================

    #[test]
    fn test_parse_simple_equation() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = 5 ~ Units ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, units, section_end) = result.unwrap();
        assert!(matches!(eq, Equation::Regular(_, _)));
        assert!(units.is_some());
        assert!(matches!(section_end, SectionEnd::Tilde));
    }

    #[test]
    fn test_parse_empty_rhs() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "placeholder = ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        assert!(matches!(eq, Equation::EmptyRhs(_, _)));
    }

    #[test]
    fn test_parse_implicit() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "exogenous data ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        assert!(matches!(eq, Equation::Implicit(_)));
    }

    #[test]
    fn test_parse_subscript_def() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "DimA: A1, A2, A3 ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::SubscriptDef(name, def) = &eq {
            assert_eq!(name.as_ref(), "DimA");
            assert_eq!(def.elements.len(), 3);
        } else {
            panic!("Expected SubscriptDef, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_equivalence() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "DimA <-> DimB ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        assert!(matches!(eq, Equation::Equivalence(_, _, _)));
    }

    #[test]
    fn test_parse_lookup_pairs() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "table((0, 0), (1, 1), (2, 4)) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Lookup(_, table) = &eq {
            assert_eq!(table.x_vals, vec![0.0, 1.0, 2.0]);
            assert_eq!(table.y_vals, vec![0.0, 1.0, 4.0]);
        } else {
            panic!("Expected Lookup, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_lookup_legacy() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "table(0, 1, 2, 10, 20, 30) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Lookup(_, table) = &eq {
            assert_eq!(table.x_vals, vec![0.0, 1.0, 2.0]);
            assert_eq!(table.y_vals, vec![10.0, 20.0, 30.0]);
        } else {
            panic!("Expected Lookup, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_with_lookup() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "y = WITH LOOKUP(Time, ((0, 0), (1, 1))) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        assert!(matches!(eq, Equation::WithLookup(_, _, _)));
    }

    #[test]
    fn test_parse_data_equation() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "data var := GET XLS DATA('file.xlsx', 'Sheet1', 'A', 'B2') ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        assert!(matches!(eq, Equation::Data(_, _)));
    }

    #[test]
    fn test_parse_number_list() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = 1, 2, 3 ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::NumberList(_, nums) = &eq {
            assert_eq!(nums, &vec![1.0, 2.0, 3.0]);
        } else {
            panic!("Expected NumberList, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_function_call() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = MAX(a, b) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Regular(_, Expr::App(name, _, args, kind, _)) = &eq {
            assert_eq!(name.as_ref(), "MAX");
            assert_eq!(args.len(), 2);
            assert_eq!(*kind, CallKind::Builtin);
        } else {
            panic!("Expected function call, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_expression_precedence() {
        use crate::mdl::normalizer::TokenNormalizer;

        // a + b * c should parse as a + (b * c)
        let input = "x = a + b * c ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Regular(_, Expr::Op2(BinaryOp::Add, _, rhs, _)) = &eq {
            assert!(
                matches!(rhs.as_ref(), Expr::Op2(BinaryOp::Mul, _, _, _)),
                "RHS should be multiplication"
            );
        } else {
            panic!("Expected Add(_, Mul(_, _)), got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_units_with_range() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = 5 ~ widgets [0, 100] ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (_, units, _) = result.unwrap();
        let units = units.unwrap();
        assert!(units.range.is_some());
        let range = units.range.unwrap();
        assert_eq!(range.min, Some(0.0));
        assert_eq!(range.max, Some(100.0));
    }

    #[test]
    fn test_parse_except_clause() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "var[DimA] :EXCEPT: [A1, A2] = 5 ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Regular(lhs, _) = &eq {
            assert!(lhs.except.is_some());
        } else {
            panic!("Expected Regular with except, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_pipe_section_end() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = 5 ~ Units |";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (_, _, section_end) = result.unwrap();
        assert!(matches!(section_end, SectionEnd::Pipe));
    }

    #[test]
    fn test_parse_eq_end() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "\\\\\\---///";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (_, _, section_end) = result.unwrap();
        assert!(matches!(section_end, SectionEnd::EqEnd(_)));
    }

    #[test]
    fn test_parse_interp_mode() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "data var :INTERPOLATE: ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Implicit(lhs) = &eq {
            assert_eq!(lhs.interp_mode, Some(InterpMode::Interpolate));
        } else {
            panic!("Expected Implicit, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_subscript_range() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "DimA: (A1 - A10) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::SubscriptDef(_, def) = &eq {
            assert_eq!(def.elements.len(), 1);
            assert!(matches!(&def.elements[0], SubscriptElement::Range(_, _, _)));
        } else {
            panic!("Expected SubscriptDef, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_subscript_mapping() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "DimA: A1, A2 -> DimB ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::SubscriptDef(_, def) = &eq {
            assert!(def.mapping.is_some());
        } else {
            panic!("Expected SubscriptDef, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_trailing_comma_function() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = SMOOTH(input, delay,) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Regular(_, Expr::App(_, _, args, _, _)) = &eq {
            assert_eq!(args.len(), 3);
            assert!(matches!(&args[2], Expr::Literal(lit, _) if lit.as_ref() == "?"));
        } else {
            panic!("Expected function call with trailing comma, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_empty_function() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = RANDOM 0 1() ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Regular(_, Expr::App(name, _, args, _, _)) = &eq {
            assert_eq!(name.as_ref(), "RANDOM 0 1");
            assert_eq!(args.len(), 0);
        } else {
            panic!("Expected empty function call, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_lookup_with_range() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "table([(0, 0) - (10, 10)], (0, 0), (5, 5), (10, 10)) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Lookup(_, table) = &eq {
            assert!(table.x_range.is_some());
            assert_eq!(table.x_vals, vec![0.0, 5.0, 10.0]);
        } else {
            panic!("Expected Lookup, got {:?}", eq);
        }
    }

    #[test]
    fn test_parse_no_units() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = 5 ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (_, units, _) = result.unwrap();
        assert!(units.is_none());
    }

    #[test]
    fn test_parse_macro_start() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = ":MACRO: MYFUNC(arg1, arg2)";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (_, _, section_end) = result.unwrap();
        if let SectionEnd::MacroStart(name, args, _) = &section_end {
            assert_eq!(name.as_ref(), "MYFUNC");
            assert_eq!(args.len(), 2);
        } else {
            panic!("Expected MacroStart, got {:?}", section_end);
        }
    }

    #[test]
    fn test_parse_macro_end() {
        use crate::mdl::normalizer::TokenNormalizer;

        let input = ":END OF MACRO:";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (_, _, section_end) = result.unwrap();
        assert!(matches!(section_end, SectionEnd::MacroEnd(_)));
    }

    #[test]
    fn test_parse_trailing_token_error() {
        // Tokens that would be valid but have extra trailing content
        let tokens = vec![
            (0, Token::EqEnd, 9),
            (10, Token::Symbol(Cow::Borrowed("extra")), 15),
        ];
        let err = parse(&tokens).unwrap_err();
        assert_eq!(err.start, 10);
        assert_eq!(err.end, 15);
        assert!(err.message.contains("trailing"));
    }

    // ========================================================================
    // Regression: reject empty-arg symbol calls (Issue #1)
    // ========================================================================

    #[test]
    fn test_reject_empty_arg_symbol_call() {
        // The LALRPOP grammar required ExprList (at least one arg) for symbol
        // calls. var() (empty parens on a non-builtin symbol) must be a parse
        // error, not an Expr::App with CallKind::Symbol and zero args.
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = table() ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_err(), "empty-arg symbol call should be rejected");
    }

    #[test]
    fn test_symbol_call_with_args_still_works() {
        // Symbol calls with at least one argument must still parse.
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "x = table(Time) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Regular(_, Expr::App(name, _, args, kind, _)) = &eq {
            assert_eq!(name.as_ref(), "table");
            assert_eq!(args.len(), 1);
            assert_eq!(*kind, CallKind::Symbol);
        } else {
            panic!("Expected symbol call, got {:?}", eq);
        }
    }

    // ========================================================================
    // Regression: validate embedded pairs in range prefix (Issue #2)
    // ========================================================================

    #[test]
    fn test_embedded_pairs_malformed_rejected() {
        // The LALRPOP grammar parsed embedded pairs as TablePairs, so malformed
        // inner pairs should produce a parse error, not be silently skipped.
        use crate::mdl::normalizer::TokenNormalizer;

        // Malformed: inner pairs have missing comma between x and y
        let input = "table([(0, 0) - (10, 10), (1 2)], (0, 0), (10, 10)) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(
            result.is_err(),
            "malformed embedded pairs should be rejected"
        );
    }

    #[test]
    fn test_embedded_pairs_wellformed_accepted() {
        // Valid embedded pairs variant should still parse.
        use crate::mdl::normalizer::TokenNormalizer;

        let input = "table([(0, 0) - (10, 10), (0, 0), (10, 10)], (0, 0), (5, 5), (10, 10)) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_ok(), "parse failed: {:?}", result.unwrap_err());
        let (eq, _, _) = result.unwrap();
        if let Equation::Lookup(_, table) = &eq {
            assert!(table.x_range.is_some());
            // The outer pairs are what matter
            assert_eq!(table.x_vals, vec![0.0, 5.0, 10.0]);
            assert_eq!(table.y_vals, vec![0.0, 5.0, 10.0]);
        } else {
            panic!("Expected Lookup, got {:?}", eq);
        }
    }

    // ========================================================================
    // Regression: legacy XY transform error span (Issue #3)
    // ========================================================================

    #[test]
    fn test_legacy_xy_transform_error_span_covers_lhs() {
        // Legacy XY errors should span from the LHS start to the closing paren,
        // matching the LALRPOP grammar's @L..@R behavior.
        use crate::mdl::normalizer::TokenNormalizer;

        // 3 values => odd count => transform_legacy fails
        let input = "table(1, 2, 3) ~ ~";
        let normalizer = TokenNormalizer::new(input);
        let tokens = collect_tokens(normalizer);
        let result = parse(&tokens);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // The error span should start at "table" (position 0), not at "(" (position 5)
        assert_eq!(
            err.start, 0,
            "error span should start at LHS, got {}",
            err.start
        );
    }
}
