// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! TokenNormalizer: Context-sensitive token transformations for Vensim MDL.
//!
//! This module sits between the context-free RawLexer and the LALRPOP parser,
//! applying transformations that depend on tracking section state (equation/units/comment).

// This module is staged for integration with the LALRPOP parser.
// The types are public to allow use by the parser module.

use std::borrow::Cow;
use std::iter::Peekable;

use crate::mdl::builtins::{is_builtin, is_tabbed_array, is_with_lookup, to_lower_space};
use crate::mdl::lexer::{LexError, LexErrorCode, RawLexer, RawToken, Spanned};

/// Normalized tokens ready for parsing.
///
/// These tokens have been transformed based on context (equation/units/comment section).
#[derive(Clone, Debug, PartialEq)]
pub enum Token<'input> {
    // Pass-through from RawToken
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
    Number(Cow<'input, str>),
    Symbol(Cow<'input, str>),
    Literal(Cow<'input, str>),
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
    GroupStar(Cow<'input, str>),
    Question, // ? for unit ranges

    // Context-dependent normalized variants
    /// Symbol in units section (may start with $)
    UnitsSymbol(Cow<'input, str>),
    /// Known builtin function
    Function(Cow<'input, str>),
    /// "WITH LOOKUP" keyword (any spacing variant via canonicalization)
    WithLookup,
    /// Parsed TABBED ARRAY data (flattened to match xmutil behavior)
    TabbedArray(Vec<f64>),
}

/// Normalizer error codes for context-sensitive validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NormalizerErrorCode {
    /// $ symbol found outside units section
    DollarSymbolOutsideUnits,
    /// Malformed numbers in TABBED ARRAY
    MalformedTabbedArray,
    /// EOF before closing ) in TABBED ARRAY
    UnclosedTabbedArray,
    /// EOF before matching ) in GET XLS/VDF
    UnclosedGetXls,
    /// Lexer error passed through
    LexError(LexErrorCode),
    /// Semantic error during parsing (e.g., mixed expression list)
    SemanticError(String),
}

/// Error from the normalizer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizerError {
    pub start: usize,
    pub end: usize,
    pub code: NormalizerErrorCode,
}

impl From<LexError> for NormalizerError {
    fn from(e: LexError) -> Self {
        NormalizerError {
            start: e.start,
            end: e.end,
            code: NormalizerErrorCode::LexError(e.code),
        }
    }
}

/// Tracks which section of an equation we're in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    /// Before first ~ (the equation itself)
    Equation,
    /// After first ~, before second ~ (units specification)
    Units,
    /// After second ~, before | (comment)
    Comment,
}

/// TokenNormalizer wraps a RawLexer to add context-sensitive transformations.
///
/// It tracks the current section (equation/units/comment) and transforms tokens:
/// - Symbol -> UnitsSymbol when in units section with units_mode active
/// - Symbol -> Function when in equation section and name is a builtin
/// - Symbol -> WithLookup for exact "WITH LOOKUP" match
/// - Handles TABBED ARRAY and GET XLS/VDF special constructs
pub struct TokenNormalizer<'input> {
    inner: Peekable<RawLexer<'input>>,
    section: Section,
    /// Within Units section, turns false when we see [
    in_units_mode: bool,
    /// Original source text for span-based parsing
    source: &'input str,
    /// Byte offset to add to all positions (for substring normalization)
    offset: usize,
}

impl<'input> TokenNormalizer<'input> {
    pub fn new(input: &'input str) -> Self {
        Self::with_offset(input, 0)
    }

    /// Create a normalizer that starts at a given byte offset in the original source.
    ///
    /// This is used when parsing subsequent equations after comments - we create
    /// a fresh normalizer for the remaining source, but positions need to be
    /// adjusted by the offset so error messages reference the correct locations.
    pub fn with_offset(input: &'input str, offset: usize) -> Self {
        TokenNormalizer {
            inner: RawLexer::new(input).peekable(),
            section: Section::Equation,
            in_units_mode: false,
            source: input,
            offset,
        }
    }

    /// Check if a symbol is a GET XLS/VDF function and return the prefix if so.
    /// Uses to_lower_space canonicalization so "GET_XLS", "GET  XLS", etc. all match.
    fn is_get_xls_or_vdf(name: &str) -> Option<&'static str> {
        let canonical = to_lower_space(name);
        if let Some(rest) = canonical.strip_prefix("get ") {
            if rest.starts_with("123") {
                return Some("{GET 123");
            }
            if rest.starts_with("data") {
                return Some("{GET DATA");
            }
            if rest.starts_with("direct") {
                return Some("{GET DIRECT");
            }
            if rest.starts_with("vdf") {
                return Some("{GET VDF");
            }
            if rest.starts_with("xls") {
                return Some("{GET XLS");
            }
        }
        None
    }

    /// Read a TABBED ARRAY construct.
    /// Called after we've seen a Symbol that canonicalizes to "tabbed array".
    /// `keyword_start` is the start position of the "TABBED ARRAY" keyword.
    /// Read a TABBED ARRAY construct.
    ///
    /// Returns a flat vector of values, matching xmutil behavior which discards
    /// row boundaries (see `ExpressionNumberTable::AddValue()` - the `row`
    /// parameter is ignored).
    fn read_tabbed_array(
        &mut self,
        keyword_start: usize,
    ) -> Result<Spanned<Token<'input>>, NormalizerError> {
        // Skip to opening (
        loop {
            match self.inner.next() {
                Some(Ok((_, RawToken::LParen, _))) => break,
                Some(Ok((pos, RawToken::Tilde, end))) => {
                    return Err(NormalizerError {
                        start: pos,
                        end,
                        code: NormalizerErrorCode::UnclosedTabbedArray,
                    });
                }
                Some(Err(e)) => return Err(e.into()),
                None => {
                    return Err(NormalizerError {
                        start: keyword_start,
                        end: self.source.len(),
                        code: NormalizerErrorCode::UnclosedTabbedArray,
                    });
                }
                // Skip whitespace, newlines, other tokens before (
                _ => continue,
            }
        }

        // Collect all values into a flat vector (xmutil ignores row boundaries)
        let mut values: Vec<f64> = Vec::new();

        loop {
            match self.inner.next() {
                Some(Ok((_, RawToken::RParen, end))) => {
                    return Ok((keyword_start, Token::TabbedArray(values), end));
                }
                Some(Ok((_, RawToken::Newline, _))) => {
                    // Newlines are ignored (just whitespace between values)
                }
                Some(Ok((_, RawToken::Number(n), _))) => {
                    // RawLexer only emits valid Number tokens, so parse should never fail
                    let val: f64 = n.parse().unwrap();
                    values.push(val);
                }
                Some(Ok((sign_pos, RawToken::Plus, sign_end))) => {
                    // C++ treats newline as whitespace after a sign, so skip Newlines
                    // until we find the number
                    match self.skip_newlines_and_get_number()? {
                        Some(val) => values.push(val),
                        None => {
                            return Err(NormalizerError {
                                start: sign_pos,
                                end: sign_end,
                                code: NormalizerErrorCode::MalformedTabbedArray,
                            });
                        }
                    }
                }
                Some(Ok((sign_pos, RawToken::Minus, sign_end))) => {
                    // C++ treats newline as whitespace after a sign, so skip Newlines
                    // until we find the number
                    match self.skip_newlines_and_get_number()? {
                        Some(val) => values.push(-val),
                        None => {
                            return Err(NormalizerError {
                                start: sign_pos,
                                end: sign_end,
                                code: NormalizerErrorCode::MalformedTabbedArray,
                            });
                        }
                    }
                }
                Some(Ok((pos, _, end))) => {
                    return Err(NormalizerError {
                        start: pos,
                        end,
                        code: NormalizerErrorCode::MalformedTabbedArray,
                    });
                }
                Some(Err(e)) => return Err(e.into()),
                None => {
                    return Err(NormalizerError {
                        start: keyword_start,
                        end: self.source.len(),
                        code: NormalizerErrorCode::UnclosedTabbedArray,
                    });
                }
            }
        }
    }

    /// Skip any Newline tokens and return the next Number value.
    /// Used in tabbed array parsing where C++ treats newline as whitespace after a sign.
    fn skip_newlines_and_get_number(&mut self) -> Result<Option<f64>, NormalizerError> {
        loop {
            match self.inner.next() {
                Some(Ok((_, RawToken::Newline, _))) => {
                    // Skip newlines after sign
                    continue;
                }
                Some(Ok((_, RawToken::Number(n), _))) => {
                    let val: f64 = n.parse().unwrap();
                    return Ok(Some(val));
                }
                Some(Ok(_)) => {
                    // Non-number, non-newline token - error
                    return Ok(None);
                }
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(None),
            }
        }
    }

    /// Read a GET XLS/VDF construct.
    /// Consumes through the closing ) and returns a Symbol containing the whole construct.
    fn read_get_xls(
        &mut self,
        prefix: &'static str,
        start: usize,
    ) -> Result<Spanned<Token<'input>>, NormalizerError> {
        let mut result = prefix.to_string();

        // Skip to opening (
        let mut actual_end = loop {
            match self.inner.next() {
                Some(Ok((_, RawToken::LParen, end))) => {
                    result.push('(');
                    break end;
                }
                Some(Err(e)) => return Err(e.into()),
                None => {
                    return Err(NormalizerError {
                        start,
                        end: self.source.len(),
                        code: NormalizerErrorCode::UnclosedGetXls,
                    });
                }
                // Skip whitespace tokens before (
                Some(Ok((_, _, _))) => {}
            }
        };

        // Track nesting and consume until matching )
        let mut nesting = 1;
        while nesting > 0 {
            match self.inner.next() {
                Some(Ok((_, RawToken::LParen, end))) => {
                    nesting += 1;
                    result.push('(');
                    actual_end = end;
                }
                Some(Ok((_, RawToken::RParen, end))) => {
                    nesting -= 1;
                    result.push(')');
                    actual_end = end;
                }
                Some(Ok((pos, _tok, end))) => {
                    // Append the token to result
                    result.push_str(&self.source[pos..end]);
                    actual_end = end;
                }
                Some(Err(e)) => return Err(e.into()),
                None => {
                    return Err(NormalizerError {
                        start,
                        end: self.source.len(),
                        code: NormalizerErrorCode::UnclosedGetXls,
                    });
                }
            }
        }

        result.push('}');
        Ok((start, Token::Symbol(Cow::Owned(result)), actual_end))
    }

    /// Transform a raw token based on current section state.
    fn transform(
        &mut self,
        spanned: Spanned<RawToken<'input>>,
    ) -> Result<Option<Spanned<Token<'input>>>, NormalizerError> {
        let (start, raw, end) = spanned;

        // Handle section transitions first
        match &raw {
            RawToken::Tilde => {
                match self.section {
                    Section::Equation => {
                        self.section = Section::Units;
                        self.in_units_mode = true;
                    }
                    Section::Units => {
                        self.section = Section::Comment;
                        self.in_units_mode = false;
                    }
                    Section::Comment => {
                        // Extra tildes in comment section are ignored
                    }
                }
                return Ok(Some((start, Token::Tilde, end)));
            }
            RawToken::Pipe => {
                self.section = Section::Equation;
                self.in_units_mode = false;
                return Ok(Some((start, Token::Pipe, end)));
            }
            RawToken::LBracket => {
                if self.section == Section::Units {
                    self.in_units_mode = false;
                }
                return Ok(Some((start, Token::LBracket, end)));
            }
            RawToken::Newline => {
                // Newlines are consumed by normalizer (used for TABBED ARRAY tracking)
                // They are not passed through to the parser
                return Ok(None);
            }
            _ => {}
        }

        // Transform based on section and token type
        let token = match raw {
            RawToken::Symbol(name) => {
                // Check for $ prefix outside units
                if name.starts_with('$') && self.section != Section::Units {
                    return Err(NormalizerError {
                        start,
                        end,
                        code: NormalizerErrorCode::DollarSymbolOutsideUnits,
                    });
                }

                if self.section == Section::Units && self.in_units_mode {
                    // In units mode, symbols become UnitsSymbol
                    Token::UnitsSymbol(name)
                } else if self.section == Section::Equation {
                    // Check for WITH LOOKUP (any spacing variant)
                    if is_with_lookup(&name) {
                        Token::WithLookup
                    }
                    // Check for GET XLS/VDF
                    else if let Some(prefix) = Self::is_get_xls_or_vdf(&name) {
                        return Ok(Some(self.read_get_xls(prefix, start)?));
                    }
                    // Check for TABBED ARRAY
                    else if is_tabbed_array(&name) {
                        return Ok(Some(self.read_tabbed_array(start)?));
                    }
                    // Check for builtin function
                    else if is_builtin(&name) {
                        Token::Function(name)
                    } else {
                        Token::Symbol(name)
                    }
                } else {
                    // In comment section, just pass through
                    Token::Symbol(name)
                }
            }
            RawToken::Number(n) => {
                if self.section == Section::Units && self.in_units_mode && n.as_ref() == "1" {
                    // "1" in units mode is a units symbol (dimensionless)
                    Token::UnitsSymbol(n)
                } else {
                    Token::Number(n)
                }
            }
            // Pass through all other tokens
            RawToken::Plus => Token::Plus,
            RawToken::Minus => Token::Minus,
            RawToken::Mul => Token::Mul,
            RawToken::Div => Token::Div,
            RawToken::Exp => Token::Exp,
            RawToken::Lt => Token::Lt,
            RawToken::Gt => Token::Gt,
            RawToken::Eq => Token::Eq,
            RawToken::Lte => Token::Lte,
            RawToken::Gte => Token::Gte,
            RawToken::Neq => Token::Neq,
            RawToken::LParen => Token::LParen,
            RawToken::RParen => Token::RParen,
            RawToken::LBracket => Token::LBracket,
            RawToken::RBracket => Token::RBracket,
            RawToken::Comma => Token::Comma,
            RawToken::Semicolon => Token::Semicolon,
            RawToken::Colon => Token::Colon,
            RawToken::Pipe => Token::Pipe,
            RawToken::Tilde => Token::Tilde,
            RawToken::Dot => Token::Dot,
            RawToken::Bang => Token::Bang,
            RawToken::Question => Token::Question,
            RawToken::DataEquals => Token::DataEquals,
            RawToken::Equiv => Token::Equiv,
            RawToken::MapArrow => Token::MapArrow,
            RawToken::Literal(l) => Token::Literal(l),
            RawToken::And => Token::And,
            RawToken::Or => Token::Or,
            RawToken::Not => Token::Not,
            RawToken::Na => Token::Na,
            RawToken::Macro => Token::Macro,
            RawToken::EndOfMacro => Token::EndOfMacro,
            RawToken::Except => Token::Except,
            RawToken::Interpolate => Token::Interpolate,
            RawToken::Raw => Token::Raw,
            RawToken::HoldBackward => Token::HoldBackward,
            RawToken::LookForward => Token::LookForward,
            RawToken::Implies => Token::Implies,
            RawToken::TestInput => Token::TestInput,
            RawToken::TheCondition => Token::TheCondition,
            RawToken::EqEnd => Token::EqEnd,
            RawToken::GroupStar(g) => Token::GroupStar(g),
            RawToken::Newline => unreachable!("handled above"),
        };

        Ok(Some((start, token, end)))
    }
}

impl<'input> Iterator for TokenNormalizer<'input> {
    type Item = Result<Spanned<Token<'input>>, NormalizerError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let raw = self.inner.next()?;
            match raw {
                Ok(spanned) => match self.transform(spanned) {
                    Ok(Some((start, tok, end))) => {
                        // Adjust positions by offset
                        return Some(Ok((start + self.offset, tok, end + self.offset)));
                    }
                    Ok(None) => continue, // Token was consumed (e.g., Newline)
                    Err(mut e) => {
                        // Adjust error positions by offset
                        e.start += self.offset;
                        e.end += self.offset;
                        return Some(Err(e));
                    }
                },
                Err(e) => {
                    // Adjust lexer error positions by offset
                    return Some(Err(NormalizerError {
                        start: e.start + self.offset,
                        end: e.end + self.offset,
                        code: NormalizerErrorCode::LexError(e.code),
                    }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to collect all tokens from a normalizer
    fn tokens(input: &str) -> Vec<Result<Spanned<Token<'_>>, NormalizerError>> {
        TokenNormalizer::new(input).collect()
    }

    // Helper to get just the token types from a successful lex
    fn token_types(input: &str) -> Vec<Token<'_>> {
        tokens(input)
            .into_iter()
            .filter_map(|r| r.ok().map(|(_, tok, _)| tok))
            .collect()
    }

    // ========== Phase 2: Section State Tests ==========

    #[test]
    fn test_section_starts_at_equation() {
        let toks = token_types("x");
        // x should be a Symbol (equation section)
        assert!(matches!(&toks[0], Token::Symbol(s) if s.as_ref() == "x"));
    }

    #[test]
    fn test_tilde_transitions_to_units() {
        let toks = token_types("x~y");
        // x is Symbol, y becomes UnitsSymbol
        assert!(matches!(&toks[0], Token::Symbol(s) if s.as_ref() == "x"));
        assert_eq!(toks[1], Token::Tilde);
        assert!(matches!(&toks[2], Token::UnitsSymbol(s) if s.as_ref() == "y"));
    }

    #[test]
    fn test_second_tilde_transitions_to_comment() {
        let toks = token_types("x~y~z");
        // z is in comment section, so it's a plain Symbol
        assert!(matches!(&toks[0], Token::Symbol(_)));
        assert!(matches!(&toks[2], Token::UnitsSymbol(_)));
        assert!(matches!(&toks[4], Token::Symbol(s) if s.as_ref() == "z"));
    }

    #[test]
    fn test_pipe_resets_to_equation() {
        let toks = token_types("x~y~z|a");
        // After |, a is back in equation section
        assert!(matches!(&toks[6], Token::Symbol(s) if s.as_ref() == "a"));
    }

    #[test]
    fn test_bracket_turns_off_units_mode() {
        let toks = token_types("x~Widgets[0]~");
        // Widgets is UnitsSymbol, but 0 is Number (units mode off after [)
        assert!(matches!(&toks[2], Token::UnitsSymbol(s) if s.as_ref() == "Widgets"));
        assert!(matches!(&toks[4], Token::Number(n) if n.as_ref() == "0"));
    }

    // ========== Phase 3: UnitsSymbol Classification Tests ==========

    #[test]
    fn test_symbol_in_units_becomes_units_symbol() {
        let toks = token_types("x~Year~");
        assert!(matches!(&toks[2], Token::UnitsSymbol(s) if s.as_ref() == "Year"));
    }

    #[test]
    fn test_bare_one_in_units() {
        let toks = token_types("x~1~");
        // "1" in units section is UnitsSymbol (dimensionless)
        assert!(matches!(&toks[2], Token::UnitsSymbol(n) if n.as_ref() == "1"));
    }

    #[test]
    fn test_number_after_bracket_stays_number() {
        let toks = token_types("x~Units[0,100]~");
        assert!(matches!(&toks[4], Token::Number(n) if n.as_ref() == "0"));
        assert!(matches!(&toks[6], Token::Number(n) if n.as_ref() == "100"));
    }

    // ========== Phase 4: Function Classification Tests ==========

    #[test]
    fn test_max_becomes_function() {
        let toks = token_types("MAX(a,b)");
        assert!(matches!(&toks[0], Token::Function(s) if s.as_ref() == "MAX"));
    }

    #[test]
    fn test_function_case_insensitive() {
        let toks = token_types("integ(x,0)");
        assert!(matches!(&toks[0], Token::Function(s) if s.as_ref() == "integ"));
    }

    #[test]
    fn test_function_with_underscores() {
        let toks = token_types("IF_THEN_ELSE(a,b,c)");
        assert!(matches!(&toks[0], Token::Function(s) if s.as_ref() == "IF_THEN_ELSE"));
    }

    #[test]
    fn test_function_with_spaces() {
        let toks = token_types("IF THEN ELSE(a,b,c)");
        assert!(matches!(&toks[0], Token::Function(s) if s.as_ref() == "IF THEN ELSE"));
    }

    #[test]
    fn test_non_function_stays_symbol() {
        let toks = token_types("my_variable");
        assert!(matches!(&toks[0], Token::Symbol(s) if s.as_ref() == "my_variable"));
    }

    #[test]
    fn test_function_name_in_units_becomes_units_symbol() {
        // Even builtin names become UnitsSymbol in units section
        let toks = token_types("x~MAX~");
        assert!(matches!(&toks[2], Token::UnitsSymbol(s) if s.as_ref() == "MAX"));
    }

    // ========== Phase 5: WithLookup Tests ==========

    #[test]
    fn test_with_lookup_exact() {
        let toks = token_types("WITH LOOKUP(x,y)");
        assert_eq!(toks[0], Token::WithLookup);
    }

    #[test]
    fn test_with_lookup_lowercase() {
        let toks = token_types("with lookup(x,y)");
        assert_eq!(toks[0], Token::WithLookup);
    }

    #[test]
    fn test_with_lookup_underscore() {
        // WITH_LOOKUP should match via canonicalization
        let toks = token_types("WITH_LOOKUP(x,y)");
        assert_eq!(toks[0], Token::WithLookup);
    }

    #[test]
    fn test_with_lookup_multi_space() {
        // "WITH  LOOKUP" with two spaces should also match via canonicalization
        let toks = token_types("WITH  LOOKUP(x,y)");
        assert_eq!(toks[0], Token::WithLookup);
    }

    // ========== Phase 6: TABBED ARRAY Tests ==========

    #[test]
    fn test_tabbed_array_simple() {
        // Values are flattened into a single vector (xmutil ignores row boundaries)
        let toks = token_types("TABBED ARRAY(1 2 3)");
        if let Token::TabbedArray(values) = &toks[0] {
            assert_eq!(values, &vec![1.0, 2.0, 3.0]);
        } else {
            panic!("Expected TabbedArray, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_tabbed_array_with_newlines() {
        // Newlines are just whitespace - values are flattened
        let toks = token_types("TABBED ARRAY(\n1 2\n3 4\n)");
        if let Token::TabbedArray(values) = &toks[0] {
            assert_eq!(values, &vec![1.0, 2.0, 3.0, 4.0]);
        } else {
            panic!("Expected TabbedArray, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_tabbed_array_with_signs() {
        let toks = token_types("TABBED ARRAY(-1 +2 -3)");
        if let Token::TabbedArray(values) = &toks[0] {
            assert_eq!(values, &vec![-1.0, 2.0, -3.0]);
        } else {
            panic!("Expected TabbedArray, got {:?}", toks[0]);
        }
    }

    // ========== Phase 7: GET XLS Tests ==========

    #[test]
    fn test_get_xls_becomes_symbol_placeholder() {
        let toks = token_types("GET XLS('file.xls', 'sheet', 'A1')");
        // Should be a Symbol containing the placeholder
        if let Token::Symbol(s) = &toks[0] {
            assert!(s.starts_with("{GET XLS"));
            assert!(s.ends_with("}"));
        } else {
            panic!("Expected Symbol placeholder, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_get_vdf_becomes_symbol_placeholder() {
        let toks = token_types("GET VDF('file.vdf')");
        if let Token::Symbol(s) = &toks[0] {
            assert!(s.starts_with("{GET VDF"));
        } else {
            panic!("Expected Symbol placeholder, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_get_xls_handles_nested_parens() {
        let toks = token_types("GET XLS('file.xls', foo(bar), 'A1')");
        if let Token::Symbol(s) = &toks[0] {
            assert!(s.starts_with("{GET XLS"));
            assert!(s.contains("foo(bar)"));
        } else {
            panic!("Expected Symbol placeholder, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_get_direct_data() {
        let toks = token_types("GET DIRECT DATA('file', 'sheet', 'A', 'B')");
        if let Token::Symbol(s) = &toks[0] {
            assert!(s.starts_with("{GET DIRECT"));
        } else {
            panic!("Expected Symbol placeholder, got {:?}", toks[0]);
        }
    }

    // ========== Phase 8: Colon Keyword Tests (verify C++ parity) ==========

    #[test]
    fn test_testinput_compact() {
        let toks = token_types(":TESTINPUT:");
        assert_eq!(toks[0], Token::TestInput);
    }

    #[test]
    fn test_testinput_with_space() {
        let toks = token_types(":TEST INPUT:");
        assert_eq!(toks[0], Token::TestInput);
    }

    #[test]
    fn test_thecondition_compact() {
        let toks = token_types(":THECONDITION:");
        assert_eq!(toks[0], Token::TheCondition);
    }

    #[test]
    fn test_thecondition_with_space() {
        let toks = token_types(":THE CONDITION:");
        assert_eq!(toks[0], Token::TheCondition);
    }

    // ========== Newline Handling ==========

    #[test]
    fn test_newlines_not_passed_through() {
        // Newlines should be consumed by normalizer, not passed to parser
        let toks = token_types("a\nb");
        assert_eq!(toks.len(), 2);
        assert!(matches!(&toks[0], Token::Symbol(s) if s.as_ref() == "a"));
        assert!(matches!(&toks[1], Token::Symbol(s) if s.as_ref() == "b"));
    }

    // ========== $ Unit Symbol Tests ==========

    #[test]
    fn test_dollar_symbol_in_units() {
        // x~$/Year~ should work - $ is a valid unit symbol
        let toks = token_types("x~$/Year~");
        assert!(matches!(&toks[0], Token::Symbol(s) if s.as_ref() == "x"));
        assert_eq!(toks[1], Token::Tilde);
        assert!(matches!(&toks[2], Token::UnitsSymbol(s) if s.as_ref() == "$"));
        assert_eq!(toks[3], Token::Div);
        assert!(matches!(&toks[4], Token::UnitsSymbol(s) if s.as_ref() == "Year"));
        assert_eq!(toks[5], Token::Tilde);
    }

    #[test]
    fn test_dollar_symbol_outside_units_errors() {
        // $ outside units section should error
        let result = tokens("$foo");
        assert!(result.len() == 1);
        assert!(matches!(
            &result[0],
            Err(NormalizerError {
                code: NormalizerErrorCode::DollarSymbolOutsideUnits,
                ..
            })
        ));
    }

    // ========== GET XLS Variant Tests ==========

    #[test]
    fn test_get_xls_with_underscore() {
        // GET_XLS should also be recognized (C++ KeywordMatch allows underscore)
        let toks = token_types("GET_XLS('file.xls', 'sheet', 'A1')");
        if let Token::Symbol(s) = &toks[0] {
            assert!(s.starts_with("{GET XLS"));
            assert!(s.ends_with(")}")); // Should include closing paren
        } else {
            panic!("Expected Symbol placeholder, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_get_xls_with_double_space() {
        // GET  XLS with double space should also be recognized
        let toks = token_types("GET  XLS('file.xls', 'sheet', 'A1')");
        if let Token::Symbol(s) = &toks[0] {
            assert!(s.starts_with("{GET XLS"));
        } else {
            panic!("Expected Symbol placeholder, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_get_xls_includes_closing_paren() {
        // Verify the placeholder includes the closing paren
        let toks = token_types("GET XLS('file')");
        if let Token::Symbol(s) = &toks[0] {
            assert!(
                s.ends_with(")}"),
                "Expected placeholder to end with ')}}', got: {}",
                s
            );
        } else {
            panic!("Expected Symbol placeholder, got {:?}", toks[0]);
        }
    }

    // ========== TABBED ARRAY Sign Handling Tests ==========

    #[test]
    fn test_tabbed_array_sign_not_followed_by_number_errors() {
        // Sign must be immediately followed by a number (after skipping newlines)
        let result = tokens("TABBED ARRAY(1 - )");
        // Should get an error for the sign not followed by number
        let has_error = result.iter().any(|r| {
            matches!(
                r,
                Err(NormalizerError {
                    code: NormalizerErrorCode::MalformedTabbedArray,
                    ..
                })
            )
        });
        assert!(has_error, "Expected MalformedTabbedArray error");
    }

    #[test]
    fn test_tabbed_array_sign_followed_by_newline_then_number() {
        // C++ treats newline as whitespace after a sign - the number can be on the next line
        let toks = token_types("TABBED ARRAY(-\n1 +\n2)");
        if let Token::TabbedArray(values) = &toks[0] {
            assert_eq!(values, &vec![-1.0, 2.0]);
        } else {
            panic!("Expected TabbedArray, got {:?}", toks[0]);
        }
    }

    #[test]
    fn test_tabbed_array_sign_followed_by_crlf_then_number() {
        // Same test but with CRLF line endings
        let toks = token_types("TABBED ARRAY(-\r\n1)");
        if let Token::TabbedArray(values) = &toks[0] {
            assert_eq!(values, &vec![-1.0]);
        } else {
            panic!("Expected TabbedArray, got {:?}", toks[0]);
        }
    }
}
