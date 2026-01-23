// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![allow(dead_code)]

use std::borrow::Cow;
use std::str::CharIndices;

use self::RawToken::*;

/// Raw tokens from context-free lexing.
///
/// These tokens are produced by RawLexer without any context-sensitive
/// transformations. The TokenNormalizer will transform these into the
/// final Token enum based on section state (equation/units/comment).
#[derive(Clone, Debug, PartialEq)]
pub enum RawToken<'input> {
    // Arithmetic operators
    Plus,
    Minus,
    Mul,
    Div,
    Exp,

    // Comparison operators
    Lt,
    Gt,
    Eq,
    Lte,
    Gte,
    Neq,

    // Brackets
    LParen,
    RParen,
    LBracket,
    RBracket,

    // Delimiters
    Comma,
    Semicolon,
    Colon,
    Pipe,
    Tilde,
    Dot,

    // Special
    Bang,
    Question, // ? for unit ranges

    // Compound operators
    DataEquals, // :=
    Equiv,      // <->
    MapArrow,   // ->

    // Literals
    Number(Cow<'input, str>),
    Symbol(Cow<'input, str>),
    Literal(Cow<'input, str>), // single-quoted 'literal'

    // Colon keywords
    And,          // :AND:
    Or,           // :OR:
    Not,          // :NOT:
    Na,           // :NA:
    Macro,        // :MACRO:
    EndOfMacro,   // :END OF MACRO:
    Except,       // :EXCEPT:
    Interpolate,  // :INTERPOLATE:
    Raw,          // :RAW:
    HoldBackward, // :HOLD BACKWARD:
    LookForward,  // :LOOK FORWARD:
    Implies,      // :IMPLIES:
    TestInput,    // :TESTINPUT:
    TheCondition, // :THECONDITION:

    // Special tokens
    EqEnd,                       // \\\---/// or ///---\\\
    GroupStar(Cow<'input, str>), // **** group markers with name
    Newline,                     // \n or \r\n, used by normalizer for tabbed arrays
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub start: usize,
    pub end: usize,
    pub code: LexErrorCode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexErrorCode {
    UnrecognizedToken,
    UnclosedComment,
    UnclosedQuotedSymbol,
    UnclosedLiteral,
}

pub type Spanned<T> = (usize, T, usize);

fn error<T>(code: LexErrorCode, start: usize, end: usize) -> Result<T, LexError> {
    Err(LexError { start, end, code })
}

/// Context-free lexer for Vensim MDL files.
///
/// RawLexer performs pure tokenization without tracking section state
/// (equation/units/comment). It emits `Newline` tokens instead of consuming
/// them as whitespace, which allows the TokenNormalizer to track row boundaries
/// for TABBED ARRAY parsing.
///
/// The TokenNormalizer wraps RawLexer to add context-sensitive transformations
/// like classifying symbols as functions or units symbols.
pub struct RawLexer<'input> {
    text: &'input str,
    chars: CharIndices<'input>,
    lookahead: Option<(usize, char)>,
    /// Pushback buffer for lookahead restoration (LIFO order)
    pushback: Vec<(usize, char)>,
}

/// Check if character is whitespace (matching C++ behavior: space, tab, CR, LF only)
fn is_mdl_whitespace(c: char) -> bool {
    c == ' ' || c == '\t' || c == '\r' || c == '\n'
}

impl<'input> RawLexer<'input> {
    pub fn new(input: &'input str) -> Self {
        let mut lexer = RawLexer {
            text: input,
            chars: input.char_indices(),
            lookahead: None,
            pushback: Vec::new(),
        };
        lexer.bump_raw();
        lexer
    }

    /// Returns the source text being lexed.
    pub fn source(&self) -> &'input str {
        self.text
    }

    /// Raw bump - advances to next character without handling line continuation
    fn bump_raw(&mut self) -> Option<(usize, char)> {
        if let Some(pushed) = self.pushback.pop() {
            self.lookahead = Some(pushed);
        } else {
            self.lookahead = self.chars.next();
        }
        self.lookahead
    }

    /// Bump with line continuation handling - like C++ GetNextChar
    /// When we see \ followed by newline, skip all whitespace and return next non-whitespace char
    fn bump(&mut self) -> Option<(usize, char)> {
        self.bump_raw();

        // Check for line continuation
        if let Some((_, '\\')) = self.lookahead {
            // Peek at next char to see if it's a newline
            let saved = self.lookahead;
            self.bump_raw();
            if let Some((_, c)) = self.lookahead
                && (c == '\n' || c == '\r')
            {
                // Skip whitespace until non-whitespace
                while let Some((_, c)) = self.lookahead {
                    if c != '\n' && c != '\r' && c != ' ' && c != '\t' {
                        break;
                    }
                    self.bump_raw();
                }
                return self.lookahead;
            }
            // Not a continuation - restore
            if let Some(current) = self.lookahead {
                self.pushback.push(current);
            }
            self.lookahead = saved;
        }

        self.lookahead
    }

    fn bump_n(&mut self, n: usize) -> Option<(usize, char)> {
        assert!(n > 0);
        for _ in 0..n {
            self.bump_raw();
        }
        self.lookahead
    }

    /// Push a character back onto the input stream
    fn push_back(&mut self, pos: usize, ch: char) {
        if let Some(current) = self.lookahead {
            self.pushback.push(current);
        }
        self.lookahead = Some((pos, ch));
    }

    fn peek(&self) -> Option<(usize, char)> {
        self.lookahead
    }

    #[allow(clippy::unnecessary_wraps)]
    fn consume(
        &mut self,
        i: usize,
        tok: RawToken<'input>,
        len: usize,
    ) -> Option<Result<Spanned<RawToken<'input>>, LexError>> {
        self.bump_raw();
        Some(Ok((i, tok, i + len)))
    }

    /// Consume the current character and add to buffer, returning true if there was a line continuation
    fn consume_char(&mut self, buffer: &mut String) -> bool {
        if let Some((old_pos, c)) = self.lookahead {
            buffer.push(c);
            self.bump();
            // Check if we skipped over a continuation (position jump > 1)
            if let Some((new_pos, _)) = self.lookahead {
                return new_pos > old_pos + c.len_utf8();
            }
        }
        false
    }

    fn number(&mut self, idx0: usize) -> Spanned<RawToken<'input>> {
        let mut buffer = String::new();
        let mut had_continuation = false;

        let starts_with_dot = matches!(self.lookahead, Some((_, '.')));

        if starts_with_dot {
            had_continuation |= self.consume_char(&mut buffer);
            while matches!(self.lookahead, Some((_, c)) if c.is_ascii_digit()) {
                had_continuation |= self.consume_char(&mut buffer);
            }
        } else {
            while matches!(self.lookahead, Some((_, c)) if c.is_ascii_digit()) {
                had_continuation |= self.consume_char(&mut buffer);
            }
            if matches!(self.lookahead, Some((_, '.'))) {
                had_continuation |= self.consume_char(&mut buffer);
                while matches!(self.lookahead, Some((_, c)) if c.is_ascii_digit()) {
                    had_continuation |= self.consume_char(&mut buffer);
                }
            }
        }

        // Exponent
        if matches!(self.lookahead, Some((_, c)) if c == 'e' || c == 'E') {
            had_continuation |= self.consume_char(&mut buffer);
            if matches!(self.lookahead, Some((_, c)) if c == '+' || c == '-') {
                had_continuation |= self.consume_char(&mut buffer);
            }
            while matches!(self.lookahead, Some((_, c)) if c.is_ascii_digit()) {
                had_continuation |= self.consume_char(&mut buffer);
            }
        }

        let end = match self.lookahead {
            Some((idx, _)) => idx,
            None => self.text.len(),
        };

        let token_value = if had_continuation {
            Cow::Owned(buffer)
        } else {
            Cow::Borrowed(&self.text[idx0..end])
        };

        (idx0, Number(token_value), end)
    }

    fn is_symbol_char(c: char) -> bool {
        c.is_alphanumeric()
            || c == ' '
            || c == '_'
            || c == '$'
            || c == '\t'
            || c == '\''
            || c as u32 > 127
    }

    fn symbol(&mut self, idx0: usize) -> Spanned<RawToken<'input>> {
        let mut buffer = String::new();
        let mut had_continuation = false;

        while matches!(self.lookahead, Some((_, c)) if Self::is_symbol_char(c)) {
            had_continuation |= self.consume_char(&mut buffer);
        }

        let end = match self.lookahead {
            Some((idx, _)) => idx,
            None => self.text.len(),
        };

        // Strip trailing spaces and underscores
        while buffer.ends_with(' ') || buffer.ends_with('_') {
            buffer.pop();
        }

        // Calculate trimmed_end for span
        let trimmed_len = buffer.len();
        let trimmed_end = if had_continuation {
            end // When there's continuation, span extends to current position
        } else {
            idx0 + trimmed_len
        };

        let token_value = if had_continuation {
            Cow::Owned(buffer)
        } else {
            Cow::Borrowed(&self.text[idx0..idx0 + trimmed_len])
        };

        (idx0, Symbol(token_value), trimmed_end)
    }

    fn quoted_symbol(&mut self, idx0: usize) -> Result<Spanned<RawToken<'input>>, LexError> {
        let mut buffer = String::new();
        let mut had_continuation = false;

        // Consume opening quote
        had_continuation |= self.consume_char(&mut buffer);

        let mut len = 1;
        loop {
            match self.lookahead {
                None => {
                    return error(LexErrorCode::UnclosedQuotedSymbol, idx0, self.text.len());
                }
                Some((idx, '"')) => {
                    had_continuation |= self.consume_char(&mut buffer);
                    let end = idx + 1;
                    let token_value = if had_continuation {
                        Cow::Owned(buffer)
                    } else {
                        Cow::Borrowed(&self.text[idx0..end])
                    };
                    return Ok((idx0, Symbol(token_value), end));
                }
                Some((_, '\\')) => {
                    // Escape sequence - consume backslash and following char
                    had_continuation |= self.consume_char(&mut buffer);
                    if self.lookahead.is_some() {
                        had_continuation |= self.consume_char(&mut buffer);
                    }
                    len += 2;
                }
                Some(_) => {
                    had_continuation |= self.consume_char(&mut buffer);
                    len += 1;
                }
            }
            if len > 1024 {
                return error(LexErrorCode::UnclosedQuotedSymbol, idx0, self.text.len());
            }
        }
    }

    fn literal(&mut self, idx0: usize) -> Result<Spanned<RawToken<'input>>, LexError> {
        let mut buffer = String::new();
        let mut had_continuation = false;

        // Consume opening quote
        had_continuation |= self.consume_char(&mut buffer);

        loop {
            match self.lookahead {
                None => return error(LexErrorCode::UnclosedLiteral, idx0, self.text.len()),
                Some((idx, '\'')) => {
                    had_continuation |= self.consume_char(&mut buffer);
                    let end = idx + 1;
                    let token_value = if had_continuation {
                        Cow::Owned(buffer)
                    } else {
                        Cow::Borrowed(&self.text[idx0..end])
                    };
                    return Ok((idx0, Literal(token_value), end));
                }
                Some(_) => {
                    had_continuation |= self.consume_char(&mut buffer);
                }
            }
        }
    }

    fn skip_comment(&mut self) -> Result<(), LexError> {
        let idx0 = match self.peek() {
            Some((idx, _)) => idx,
            None => self.text.len(),
        };

        let mut nesting = 1;
        loop {
            match self.lookahead {
                None => return error(LexErrorCode::UnclosedComment, idx0, self.text.len()),
                Some((_, '{')) => {
                    nesting += 1;
                    self.bump();
                }
                Some((_, '}')) => {
                    nesting -= 1;
                    self.bump();
                    if nesting == 0 {
                        return Ok(());
                    }
                }
                Some(_) => {
                    self.bump();
                }
            }
        }
    }

    fn colon_keyword(&mut self, idx0: usize) -> Spanned<RawToken<'input>> {
        // We're at the colon, consume it
        self.bump();

        // Check for := first
        if let Some((_, '=')) = self.lookahead {
            self.bump();
            return (idx0, DataEquals, idx0 + 2);
        }

        // Get the first character after the colon
        let first_char = match self.lookahead {
            Some((_, c)) if c.is_alphabetic() => c.to_ascii_uppercase(),
            _ => return (idx0, Colon, idx0 + 1),
        };

        // List of colon keywords and their tokens
        // Note: keywords here have spaces replaced with _ for matching purposes
        // The actual matching handles spaces, underscores, and tabs as equivalent
        // C++ uses compact forms for TEST INPUT and THE CONDITION
        static KEYWORDS: &[(&str, RawToken<'static>)] = &[
            ("AND:", And),
            ("END OF MACRO:", EndOfMacro),
            ("EXCEPT:", Except),
            ("HOLD BACKWARD:", HoldBackward),
            ("IMPLIES:", Implies),
            ("INTERPOLATE:", Interpolate),
            ("LOOK FORWARD:", LookForward),
            ("MACRO:", Macro),
            ("OR:", Or),
            ("NA:", Na),
            ("NOT:", Not),
            ("RAW:", Raw),
            ("TEST INPUT:", TestInput),
            ("TESTINPUT:", TestInput),
            ("THE CONDITION:", TheCondition),
            ("THECONDITION:", TheCondition),
        ];

        // Try each keyword that starts with the same letter
        for (keyword, token) in KEYWORDS.iter() {
            if keyword.chars().next().unwrap() == first_char
                && let Some(end) = self.try_keyword_match(keyword)
            {
                return (idx0, token.clone(), end);
            }
        }

        // No keyword matched, return just the colon
        (idx0, Colon, idx0 + 1)
    }

    /// Try to match a colon keyword (without the leading :).
    /// On success, consumes the input and returns the end byte position.
    /// On failure, pushes back consumed characters and returns None.
    fn try_keyword_match(&mut self, target: &str) -> Option<usize> {
        let mut consumed: Vec<(usize, char)> = Vec::new();

        for target_char in target.chars() {
            let Some((pos, c)) = self.lookahead else {
                // EOF before matching - restore consumed chars and return None
                self.restore_chars(consumed);
                return None;
            };

            if target_char == ' ' {
                // Space in keyword can match space, underscore, or tab (one or more)
                if c != ' ' && c != '_' && c != '\t' {
                    // Not a space-like char where expected
                    self.restore_chars(consumed);
                    return None;
                }
                consumed.push((pos, c));
                self.bump();
                // Consume additional space-like chars
                while let Some((pos2, c2)) = self.lookahead {
                    if c2 == ' ' || c2 == '_' || c2 == '\t' {
                        consumed.push((pos2, c2));
                        self.bump();
                    } else {
                        break;
                    }
                }
            } else if c.to_ascii_uppercase() != target_char {
                // Character mismatch
                self.restore_chars(consumed);
                return None;
            } else {
                consumed.push((pos, c));
                self.bump();
            }
        }

        // Return the byte position after the last consumed character
        let end = consumed
            .last()
            .map(|(pos, c)| pos + c.len_utf8())
            .unwrap_or(0);
        Some(end)
    }

    /// Restore consumed characters back to the input stream
    fn restore_chars(&mut self, mut chars: Vec<(usize, char)>) {
        // Push back in reverse order (LIFO)
        while let Some((pos, ch)) = chars.pop() {
            self.push_back(pos, ch);
        }
    }

    fn check_eq_end(
        &mut self,
        idx0: usize,
        pattern1: &str,
        pattern2: &str,
    ) -> Option<Spanned<RawToken<'input>>> {
        // Check if the text starting at current position matches either pattern
        let remaining = &self.text[idx0..];

        for pattern in [pattern1, pattern2] {
            if remaining.starts_with(pattern) {
                // Consume the entire pattern
                self.bump_n(pattern.len());
                return Some((idx0, EqEnd, idx0 + pattern.len()));
            }
        }
        None
    }

    /// Handle comments, including {**group**} markers
    fn handle_comment(
        &mut self,
        idx0: usize,
    ) -> Result<Option<Spanned<RawToken<'input>>>, LexError> {
        let mut nesting = 1;
        let mut len = 1;

        loop {
            match self.lookahead {
                None => {
                    return Err(LexError {
                        start: idx0,
                        end: self.text.len(),
                        code: LexErrorCode::UnclosedComment,
                    });
                }
                Some((_, '{')) => {
                    nesting += 1;
                    self.bump_raw();
                    len += 1;
                }
                Some((_, '}')) => {
                    nesting -= 1;
                    self.bump_raw();
                    len += 1;
                    if nesting == 0 {
                        return Ok(None); // Regular comment, no group
                    }
                }
                Some((_, '*')) if nesting == 1 => {
                    self.bump_raw();
                    // Check for ** which starts a group marker
                    if let Some((_, '*')) = self.lookahead {
                        // This is a {**group**} marker
                        // Skip all leading *
                        while let Some((_, '*')) = self.lookahead {
                            self.bump_raw();
                        }
                        // Skip whitespace
                        while let Some((_, c)) = self.lookahead {
                            if c == '\r' || c == '\n' || c == ' ' || c == '\t' {
                                self.bump_raw();
                            } else {
                                break;
                            }
                        }
                        // Check for empty group (just closing })
                        if let Some((_, '}')) = self.lookahead {
                            self.bump_raw();
                            return Ok(None); // Empty group, treat as comment
                        }
                        // Collect group name, normalizing . to -
                        let mut name = String::new();
                        loop {
                            match self.lookahead {
                                None => {
                                    return Err(LexError {
                                        start: idx0,
                                        end: self.text.len(),
                                        code: LexErrorCode::UnclosedComment,
                                    });
                                }
                                Some((_, c)) if c == '\r' || c == '\n' || c == '*' || c == '}' => {
                                    break;
                                }
                                Some((_, '.')) => {
                                    // Normalize . to - (can't use . in a module name)
                                    if !name.is_empty() {
                                        name.push('-');
                                    }
                                    self.bump_raw();
                                }
                                Some((_, c)) => {
                                    name.push(c);
                                    self.bump_raw();
                                }
                            }
                        }
                        // Strip trailing spaces
                        while name.ends_with(' ') {
                            name.pop();
                        }
                        // Skip to closing }
                        while let Some((_, c)) = self.lookahead {
                            self.bump_raw();
                            if c == '}' {
                                break;
                            }
                        }
                        let end = match self.lookahead {
                            Some((idx, _)) => idx,
                            None => self.text.len(),
                        };
                        return Ok(Some((idx0, GroupStar(Cow::Owned(name)), end)));
                    } else {
                        len += 1;
                    }
                }
                Some(_) => {
                    self.bump_raw();
                    len += 1;
                }
            }
            // C++ has a 1028 char limit on comments
            if len > 1028 {
                break;
            }
        }

        // Give up on overly long comments
        Err(LexError {
            start: idx0,
            end: self.text.len(),
            code: LexErrorCode::UnclosedComment,
        })
    }

    /// Try to parse *** group marker (C++ requires *** i.e. 3 stars minimum)
    fn try_group_star(&mut self, idx0: usize) -> Option<Spanned<RawToken<'input>>> {
        // Check for *** (3 stars) - C++ checks for "**" after seeing first star
        let remaining = &self.text[idx0..];
        if !remaining.starts_with("***") {
            return None;
        }

        // Consume all leading *
        while let Some((_, '*')) = self.lookahead {
            self.bump_raw();
        }

        // Skip whitespace and newlines
        while let Some((_, c)) = self.lookahead {
            if c == '\r' || c == '\n' || c == ' ' || c == '\t' {
                self.bump_raw();
            } else {
                break;
            }
        }

        // Collect group name, normalizing . to -
        let mut name = String::new();
        loop {
            match self.lookahead {
                None => break,
                Some((_, c)) if c == '\r' || c == '\n' || c == ' ' || c == '\t' => {
                    break;
                }
                Some((_, '.')) => {
                    // Normalize . to - (can't use . in a module name)
                    if !name.is_empty() {
                        name.push('-');
                    }
                    self.bump_raw();
                }
                Some((_, c)) => {
                    name.push(c);
                    self.bump_raw();
                }
            }
        }

        // Skip to * (start of closing ***) or |
        while let Some((_, c)) = self.lookahead {
            if c == '*' || c == '|' {
                break;
            }
            self.bump_raw();
        }

        // Skip closing *** and any remaining * until | (but don't consume |)
        while let Some((_, c)) = self.lookahead {
            if c == '|' {
                break;
            }
            self.bump_raw();
        }

        // C++ consumes through | - consume it too
        let end = if let Some((idx, '|')) = self.lookahead {
            self.bump_raw();
            idx + 1
        } else {
            match self.lookahead {
                Some((idx, _)) => idx,
                None => self.text.len(),
            }
        };

        Some((idx0, GroupStar(Cow::Owned(name)), end))
    }
}

impl<'input> Iterator for RawLexer<'input> {
    type Item = Result<Spanned<RawToken<'input>>, LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            return match self.lookahead {
                Some((i, '+')) => self.consume(i, Plus, 1),
                Some((i, '*')) => {
                    // Check for *** group markers
                    if let Some(result) = self.try_group_star(i) {
                        return Some(Ok(result));
                    }
                    self.consume(i, Mul, 1)
                }
                Some((i, '/')) => {
                    // Check for ///---\\\ first
                    if let Some(result) = self.check_eq_end(i, "///---\\\\\\", "///---\\\\") {
                        return Some(Ok(result));
                    }
                    self.consume(i, Div, 1)
                }
                Some((i, '^')) => self.consume(i, Exp, 1),
                Some((i, '(')) => self.consume(i, LParen, 1),
                Some((i, ')')) => self.consume(i, RParen, 1),
                Some((i, '[')) => self.consume(i, LBracket, 1),
                Some((i, ']')) => self.consume(i, RBracket, 1),
                Some((i, ',')) => self.consume(i, Comma, 1),
                Some((i, ';')) => self.consume(i, Semicolon, 1),
                Some((i, '|')) => self.consume(i, Pipe, 1),
                Some((i, '~')) => self.consume(i, Tilde, 1),
                Some((i, '!')) => self.consume(i, Bang, 1),
                Some((i, '?')) => self.consume(i, Question, 1),
                Some((i, '=')) => {
                    self.bump_raw();
                    // == is the same as = in Vensim (invariant check), consume second =
                    let end = if let Some((_, '=')) = self.lookahead {
                        self.bump_raw();
                        i + 2
                    } else {
                        i + 1
                    };
                    Some(Ok((i, Eq, end)))
                }
                Some((i, ':')) => Some(Ok(self.colon_keyword(i))),
                Some((i, '<')) => {
                    self.bump();
                    match self.lookahead {
                        Some((j, '-')) => {
                            self.bump();
                            // Check for <->
                            match self.lookahead {
                                Some((_, '>')) => self.consume(i, Equiv, 3),
                                _ => {
                                    // Not <->, push back the - and return Lt
                                    self.push_back(j, '-');
                                    Some(Ok((i, Lt, i + 1)))
                                }
                            }
                        }
                        Some((_, '>')) => self.consume(i, Neq, 2),
                        Some((_, '=')) => self.consume(i, Lte, 2),
                        _ => Some(Ok((i, Lt, i + 1))),
                    }
                }
                Some((i, '>')) => match self.bump() {
                    Some((_, '=')) => self.consume(i, Gte, 2),
                    _ => Some(Ok((i, Gt, i + 1))),
                },
                Some((i, '-')) => match self.bump() {
                    Some((_, '>')) => self.consume(i, MapArrow, 2),
                    _ => Some(Ok((i, Minus, i + 1))),
                },
                Some((i, '\\')) => {
                    // Check for \\\---/// first
                    if let Some(result) = self.check_eq_end(i, "\\\\\\---///", "\\\\---///") {
                        return Some(Ok(result));
                    }
                    // Otherwise, check for line continuation
                    self.bump_raw();
                    if let Some((_, c)) = self.lookahead
                        && (c == '\n' || c == '\r')
                    {
                        // Line continuation - skip whitespace
                        loop {
                            match self.lookahead {
                                Some((_, c)) if c == '\n' || c == '\r' || c == ' ' || c == '\t' => {
                                    self.bump_raw();
                                }
                                _ => break,
                            }
                        }
                        continue;
                    }
                    Some(error(LexErrorCode::UnrecognizedToken, i, i + 1))
                }
                Some((i, '{')) => {
                    self.bump_raw(); // consume the opening brace
                    match self.handle_comment(i) {
                        Ok(Some(result)) => Some(Ok(result)), // GroupStar found
                        Ok(None) => continue,                 // Regular comment, skip
                        Err(e) => Some(Err(e)),
                    }
                }
                Some((i, '"')) => Some(self.quoted_symbol(i)),
                Some((i, '\'')) => Some(self.literal(i)),
                Some((i, c)) if c.is_ascii_digit() => Some(Ok(self.number(i))),
                Some((i, '.')) => {
                    self.bump_raw();
                    // Check if it's a number (. followed by digit)
                    if let Some((_, next)) = self.lookahead
                        && next.is_ascii_digit()
                    {
                        // Push back the dot and parse as number
                        self.push_back(i, '.');
                        return Some(Ok(self.number(i)));
                    }
                    // Otherwise, return Dot token
                    Some(Ok((i, Dot, i + 1)))
                }
                // $ can start a symbol (validated by normalizer to only appear in units section)
                Some((i, '$')) => Some(Ok(self.symbol(i))),
                Some((i, c)) if c.is_alphabetic() || c as u32 > 127 => Some(Ok(self.symbol(i))),
                // Emit Newline tokens for \n and \r\n (needed by normalizer for tabbed arrays)
                Some((i, '\n')) => {
                    self.bump_raw();
                    Some(Ok((i, Newline, i + 1)))
                }
                Some((i, '\r')) => {
                    self.bump_raw();
                    // Handle \r\n as single newline
                    let end = if let Some((_, '\n')) = self.lookahead {
                        self.bump_raw();
                        i + 2
                    } else {
                        i + 1
                    };
                    Some(Ok((i, Newline, end)))
                }
                // Skip spaces and tabs as whitespace
                Some((_, c)) if c == ' ' || c == '\t' => {
                    self.bump_raw();
                    continue;
                }
                Some((i, _)) => {
                    self.bump();
                    let end = match self.lookahead {
                        Some((idx, _)) => idx,
                        None => self.text.len(),
                    };
                    Some(error(LexErrorCode::UnrecognizedToken, i, end))
                }
                // EOF - return None (no implicit EqEnd, that's handled by EquationReader)
                None => None,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LexErrorCode::*;
    use super::*;

    // Helper functions to create RawToken variants with Cow::Borrowed for cleaner tests
    fn num(s: &str) -> RawToken<'_> {
        Number(Cow::Borrowed(s))
    }
    fn sym(s: &str) -> RawToken<'_> {
        Symbol(Cow::Borrowed(s))
    }
    fn lit(s: &str) -> RawToken<'_> {
        Literal(Cow::Borrowed(s))
    }
    #[allow(dead_code)]
    fn group(s: &str) -> RawToken<'_> {
        GroupStar(Cow::Borrowed(s))
    }
    // For owned strings (when normalization/continuation happens)
    fn group_owned(s: &str) -> RawToken<'static> {
        GroupStar(Cow::Owned(s.to_string()))
    }

    /// Test helper that verifies tokens match expected positions and values.
    /// Unlike the old test helper, this doesn't expect implicit EqEnd at EOF
    /// since RawLexer is context-free and doesn't emit implicit EqEnd.
    fn test(input: &str, expected: Vec<(&str, RawToken)>) {
        let tokenizer = RawLexer::new(input);
        let len = expected.len();
        for (token, (expected_span, expected_tok)) in tokenizer.zip(expected.into_iter()) {
            let expected_start = expected_span.find('~').unwrap();
            let expected_end = expected_span.rfind('~').unwrap() + 1;
            assert_eq!(Ok((expected_start, expected_tok, expected_end)), token);
        }

        // After consuming all expected tokens, next should be None
        // (RawLexer doesn't emit implicit EqEnd, that's for EquationReader)
        let mut tokenizer = RawLexer::new(input);
        let next = tokenizer.nth(len);
        // Skip any trailing newlines before checking for None
        let next = if matches!(next, Some(Ok((_, Newline, _)))) {
            let mut iter = tokenizer;
            loop {
                match iter.next() {
                    Some(Ok((_, Newline, _))) => continue,
                    other => break other,
                }
            }
        } else {
            next
        };
        assert_eq!(None, next, "Expected None at end of input, got {:?}", next);
    }

    fn test_err(input: &str, expected: (&str, LexErrorCode)) {
        let tokenizer = RawLexer::new(input);
        let mut last_err = None;
        for token in tokenizer {
            if let Err(e) = token {
                last_err = Some(e);
                break;
            }
        }
        let (expected_span, expected_code) = expected;
        let expected_start = expected_span.find('~').unwrap();
        let expected_end = expected_span.rfind('~').unwrap() + 1;
        let expected_err = LexError {
            start: expected_start,
            end: expected_end,
            code: expected_code,
        };
        assert_eq!(Some(expected_err), last_err);
    }

    // Single-character token tests
    #[test]
    fn arithmetic_operators() {
        test("+", vec![("~", Plus)]);
        test("-", vec![("~", Minus)]);
        test("*", vec![("~", Mul)]);
        test("/", vec![("~", Div)]);
        test("^", vec![("~", Exp)]);
    }

    #[test]
    fn brackets() {
        test("()", vec![("~ ", LParen), (" ~", RParen)]);
        test("[]", vec![("~ ", LBracket), (" ~", RBracket)]);
    }

    #[test]
    fn delimiters() {
        test(",", vec![("~", Comma)]);
        test(";", vec![("~", Semicolon)]);
        test(":", vec![("~", Colon)]);
        test("|", vec![("~", Pipe)]);
        test("~", vec![("~", Tilde)]);
        test("!", vec![("~", Bang)]);
    }

    #[test]
    fn comparison_operators() {
        test("<", vec![("~", Lt)]);
        test(">", vec![("~", Gt)]);
        test("=", vec![("~", Eq)]);
        test("<=", vec![("~~", Lte)]);
        test(">=", vec![("~~", Gte)]);
        test("<>", vec![("~~", Neq)]);
    }

    #[test]
    fn compound_operators() {
        test(":=", vec![("~~", DataEquals)]);
        test("<->", vec![("~~~", Equiv)]);
        test("->", vec![("~~", MapArrow)]);
    }

    // Number tests
    #[test]
    fn integers() {
        test("0", vec![("~", num("0"))]);
        test("123", vec![("~~~", num("123"))]);
        test("42", vec![("~~", num("42"))]);
    }

    #[test]
    fn floats() {
        test("1.5", vec![("~~~", num("1.5"))]);
        test(".5", vec![("~~", num(".5"))]);
        test("1.", vec![("~~", num("1."))]);
        test("3.14159", vec![("~~~~~~~", num("3.14159"))]);
    }

    #[test]
    fn scientific_notation() {
        test("1e5", vec![("~~~", num("1e5"))]);
        test("1E5", vec![("~~~", num("1E5"))]);
        test("1e-5", vec![("~~~~", num("1e-5"))]);
        test("1e+5", vec![("~~~~", num("1e+5"))]);
        test("1.5e3", vec![("~~~~~", num("1.5e3"))]);
        test("1.5E-3", vec![("~~~~~~", num("1.5E-3"))]);
        test("2.06101e+06", vec![("~~~~~~~~~~~", num("2.06101e+06"))]);
    }

    // Symbol tests
    #[test]
    fn simple_symbols() {
        test("x", vec![("~", sym("x"))]);
        test("variable", vec![("~~~~~~~~", sym("variable"))]);
        test("my_var", vec![("~~~~~~", sym("my_var"))]);
    }

    #[test]
    fn symbols_with_spaces() {
        test(
            "my variable name+",
            vec![
                ("~~~~~~~~~~~~~~~~ ", sym("my variable name")),
                ("                ~", Plus),
            ],
        );
    }

    #[test]
    fn symbols_strip_trailing() {
        // Trailing spaces and underscores are stripped
        test("var  +", vec![("~~~  ", sym("var")), ("     ~", Plus)]);
        test("var__+", vec![("~~~  ", sym("var")), ("     ~", Plus)]);
    }

    #[test]
    fn quoted_symbols() {
        // C++ returns VPTT_symbol for quoted symbols
        test(r#""a.b""#, vec![("~~~~~", sym(r#""a.b""#))]);
        test(
            r#""my variable""#,
            vec![("~~~~~~~~~~~~~", sym(r#""my variable""#))],
        );
    }

    #[test]
    fn quoted_symbols_with_escapes() {
        test(
            r#""name with \"quotes\"""#,
            vec![("~~~~~~~~~~~~~~~~~~~~~~", sym(r#""name with \"quotes\"""#))],
        );
    }

    // Literal tests
    #[test]
    fn literals() {
        test("'literal'", vec![("~~~~~~~~~", lit("'literal'"))]);
    }

    // Colon keyword tests
    #[test]
    fn colon_keywords() {
        test(":AND:", vec![("~~~~~", And)]);
        test(":OR:", vec![("~~~~", Or)]);
        test(":NOT:", vec![("~~~~~", Not)]);
        test(":NA:", vec![("~~~~", Na)]);
    }

    #[test]
    fn colon_keywords_case_insensitive() {
        test(":and:", vec![("~~~~~", And)]);
        test(":And:", vec![("~~~~~", And)]);
    }

    #[test]
    fn colon_keywords_with_spaces() {
        test(":END OF MACRO:", vec![("~~~~~~~~~~~~~~", EndOfMacro)]);
        test(":HOLD BACKWARD:", vec![("~~~~~~~~~~~~~~~", HoldBackward)]);
        test(":LOOK FORWARD:", vec![("~~~~~~~~~~~~~~", LookForward)]);
    }

    // Comment tests
    #[test]
    fn comments_skipped() {
        test(
            "a{ comment }b",
            vec![("~            ", sym("a")), ("            ~", sym("b"))],
        );
    }

    #[test]
    fn nested_comments() {
        test(
            "a{ { nested } }b",
            vec![
                ("~               ", sym("a")),
                ("               ~", sym("b")),
            ],
        );
    }

    #[test]
    fn unclosed_comment() {
        test_err("a{comment", (" ~~~~~~~~", UnclosedComment));
    }

    // Error tests
    #[test]
    fn unclosed_quoted_symbol() {
        test_err(r#""unclosed"#, ("~~~~~~~~~", UnclosedQuotedSymbol));
    }

    #[test]
    fn unclosed_literal() {
        test_err("'unclosed", ("~~~~~~~~~", UnclosedLiteral));
    }

    // Complex expressions
    #[test]
    fn simple_expression() {
        test(
            "a + b * c",
            vec![
                ("~        ", sym("a")),
                ("  ~      ", Plus),
                ("    ~    ", sym("b")),
                ("      ~  ", Mul),
                ("        ~", sym("c")),
            ],
        );
    }

    #[test]
    fn subscript_expression() {
        test(
            "var[dim]",
            vec![
                ("~~~     ", sym("var")),
                ("   ~    ", LBracket),
                ("    ~~~ ", sym("dim")),
                ("       ~", RBracket),
            ],
        );
    }

    #[test]
    fn function_call() {
        test(
            "MAX(a, b)",
            vec![
                ("~~~      ", sym("MAX")),
                ("   ~     ", LParen),
                ("    ~    ", sym("a")),
                ("     ~   ", Comma),
                ("       ~ ", sym("b")),
                ("        ~", RParen),
            ],
        );
    }

    #[test]
    fn equation_with_units() {
        test(
            "x = 5~",
            vec![
                ("~     ", sym("x")),
                ("  ~   ", Eq),
                ("    ~ ", num("5")),
                ("     ~", Tilde),
            ],
        );
    }

    // Line continuation test
    #[test]
    fn line_continuation() {
        test(
            "a + \\\nb",
            vec![
                ("~      ", sym("a")),
                ("  ~    ", Plus),
                ("      ~", sym("b")),
            ],
        );
    }

    // End marker tests
    #[test]
    fn eq_end_marker_backslash() {
        test("\\\\\\---///", vec![("~~~~~~~~~", EqEnd)]);
    }

    #[test]
    fn eq_end_marker_forward() {
        test("///---\\\\\\", vec![("~~~~~~~~~", EqEnd)]);
    }

    // ===== Bug fix tests =====

    // EOF handling - RawLexer returns None at EOF (no implicit EqEnd)
    #[test]
    fn eof_returns_none() {
        // RawLexer doesn't emit implicit EqEnd; that's handled by EquationReader
        let input = "x = 5";
        let tokens: Vec<_> = RawLexer::new(input).collect();
        assert_eq!(tokens.len(), 3); // x, =, 5
        assert_eq!(tokens[0], Ok((0, sym("x"), 1)));
        assert_eq!(tokens[1], Ok((2, Eq, 3)));
        assert_eq!(tokens[2], Ok((4, num("5"), 5)));
    }

    // Bug: <- not followed by > should preserve the -
    #[test]
    fn less_than_minus_not_equiv() {
        // <-5 should be Lt, Minus, Number, not Lt followed by loss of -
        test("<-5", vec![("~  ", Lt), (" ~ ", Minus), ("  ~", num("5"))]);
    }

    // Bug: . not followed by digit should return Dot token, not error
    #[test]
    fn standalone_dot() {
        test(
            "a.b",
            vec![("~  ", sym("a")), (" ~ ", Dot), ("  ~", sym("b"))],
        );
    }

    // Bug: == should emit single Eq token (C++ ignores second =)
    #[test]
    fn double_equals() {
        // == in Vensim is just = (invariant check, we ignore it like C++)
        test(
            "a == b",
            vec![("~    ", sym("a")), ("  ~~ ", Eq), ("     ~", sym("b"))],
        );
    }

    // RawLexer is context-free: ~ doesn't change how tokens are classified.
    // The TokenNormalizer handles units mode classification.
    #[test]
    fn tilde_in_expression() {
        // RawLexer just emits all tokens without context; 1 is a Number
        test(
            "x~1~",
            vec![
                ("~   ", sym("x")),
                (" ~  ", Tilde),
                ("  ~ ", num("1")),
                ("   ~", Tilde),
            ],
        );
    }

    // RawLexer emits Symbol for identifiers regardless of position
    #[test]
    fn symbols_after_tilde() {
        // RawLexer treats all symbols the same; normalizer handles units mode
        test(
            "x~Widgets[0,100]~",
            vec![
                ("~                ", sym("x")),
                (" ~               ", Tilde),
                ("  ~~~~~~~        ", sym("Widgets")),
                ("         ~       ", LBracket),
                ("          ~      ", num("0")),
                ("           ~     ", Comma),
                ("            ~~~  ", num("100")),
                ("               ~ ", RBracket),
                ("                ~", Tilde),
            ],
        );
    }

    // Bug: Colon keyword - :FOO: unknown keyword should return just Colon and preserve FOO
    #[test]
    fn colon_keyword_unknown_preserves_text() {
        // :FOO: is not a valid keyword, should return : and then FOO as symbol, then :
        // Input: :FOO:x (positions 0-5)
        // Position 0: :
        // Position 1-3: FOO
        // Position 4: :
        // Position 5: x
        test(
            ":FOO:x",
            vec![
                ("~     ", Colon),      // position 0
                (" ~~~  ", sym("FOO")), // positions 1-4 (exclusive end)
                ("    ~ ", Colon),      // position 4
                ("     ~", sym("x")),   // position 5
            ],
        );
    }

    // Bug: Colon keywords with underscores/tabs as spaces
    #[test]
    fn colon_keyword_with_underscore() {
        test(":HOLD_BACKWARD:", vec![("~~~~~~~~~~~~~~~", HoldBackward)]);
    }

    #[test]
    fn colon_keyword_with_multiple_underscores() {
        // When the space in the keyword matches multiple underscores,
        // the span end must be the actual consumed length, not keyword.len()
        // ":HOLD___BACKWARD:" has 17 chars (3 underscores instead of 1 space)
        test(
            ":HOLD___BACKWARD:",
            vec![("~~~~~~~~~~~~~~~~~", HoldBackward)],
        );
    }

    #[test]
    fn colon_keyword_with_tab_and_spaces() {
        // Mix of tab and spaces where keyword has single space
        // ":TEST\t INPUT:" has 14 chars (tab + 2 spaces between TEST and INPUT)
        test(":TEST\t  INPUT:", vec![("~~~~~~~~~~~~~~", TestInput)]);
    }

    // Bug: Missing colon keywords - IMPLIES, TEST INPUT, THE CONDITION
    #[test]
    fn colon_keyword_implies() {
        test(":IMPLIES:", vec![("~~~~~~~~~", Implies)]);
    }

    #[test]
    fn colon_keyword_test_input() {
        test(":TEST INPUT:", vec![("~~~~~~~~~~~~", TestInput)]);
    }

    #[test]
    fn colon_keyword_the_condition() {
        test(":THE CONDITION:", vec![("~~~~~~~~~~~~~~~", TheCondition)]);
    }

    // Bug: GroupStar - *** markers should emit GroupStar
    #[test]
    fn group_star_marker() {
        // *** group markers now consume the | terminator
        // So the entire "***\nMyGroup\n***|" is one token
        test(
            "***\nMyGroup\n***|",
            vec![("~~~~~~~~~~~~~~~~", group_owned("MyGroup"))],
        );
    }

    // Bug: {**group**} inside comment should emit GroupStar
    #[test]
    fn group_star_in_comment() {
        test(
            "{**GroupName**}",
            vec![("~~~~~~~~~~~~~~~", group_owned("GroupName"))],
        );
    }

    // Test group name normalization (. -> -)
    #[test]
    fn group_star_dot_normalization() {
        // Input is 22 chars: "***\nMy.Group.Name\n***|"
        test(
            "***\nMy.Group.Name\n***|",
            vec![("~~~~~~~~~~~~~~~~~~~~~~", group_owned("My-Group-Name"))],
        );
    }

    // Test compact :TESTINPUT: keyword (C++ accepts both forms)
    #[test]
    fn colon_keyword_testinput_compact() {
        test(":TESTINPUT:", vec![("~~~~~~~~~~~", TestInput)]);
    }

    // Test compact :THECONDITION: keyword (C++ accepts both forms)
    #[test]
    fn colon_keyword_thecondition_compact() {
        test(":THECONDITION:", vec![("~~~~~~~~~~~~~~", TheCondition)]);
    }

    // RawLexer is context-free: pipe doesn't change state
    #[test]
    fn pipe_in_equation() {
        // RawLexer emits all tokens without context
        test(
            "x~1~|y",
            vec![
                ("~     ", sym("x")),
                (" ~    ", Tilde),
                ("  ~   ", num("1")),
                ("   ~  ", Tilde),
                ("    ~ ", Pipe),
                ("     ~", sym("y")),
            ],
        );
    }

    // Test line continuation in symbol
    #[test]
    fn line_continuation_in_symbol() {
        // A symbol that spans a line continuation should be joined
        let input = "my\\\n_var";
        let tokens: Vec<_> = RawLexer::new(input).collect();
        // Should get one symbol token "my_var"
        assert_eq!(tokens.len(), 1);
        if let Ok((_, Symbol(name), _)) = &tokens[0] {
            assert_eq!(name.as_ref(), "my_var");
        } else {
            panic!("Expected Symbol token, got {:?}", tokens[0]);
        }
    }

    // Test line continuation in number
    #[test]
    fn line_continuation_in_number() {
        let input = "12\\\n34";
        let tokens: Vec<_> = RawLexer::new(input).collect();
        // Should get one number token "1234"
        assert_eq!(tokens.len(), 1);
        if let Ok((_, Number(n), _)) = &tokens[0] {
            assert_eq!(n.as_ref(), "1234");
        } else {
            panic!("Expected Number token, got {:?}", tokens[0]);
        }
    }

    // Test Newline token emission
    #[test]
    fn newline_tokens() {
        let input = "a\nb";
        let tokens: Vec<_> = RawLexer::new(input).collect();
        assert_eq!(tokens.len(), 3); // a, Newline, b
        assert_eq!(tokens[0], Ok((0, sym("a"), 1)));
        assert_eq!(tokens[1], Ok((1, Newline, 2)));
        assert_eq!(tokens[2], Ok((2, sym("b"), 3)));
    }

    #[test]
    fn crlf_newline() {
        let input = "a\r\nb";
        let tokens: Vec<_> = RawLexer::new(input).collect();
        assert_eq!(tokens.len(), 3); // a, Newline, b
        assert_eq!(tokens[0], Ok((0, sym("a"), 1)));
        assert_eq!(tokens[1], Ok((1, Newline, 3))); // \r\n spans 2 bytes
        assert_eq!(tokens[2], Ok((3, sym("b"), 4)));
    }

    // Test $ can start a symbol (for unit symbols like $/Year)
    #[test]
    fn dollar_starts_symbol() {
        let input = "$";
        let tokens: Vec<_> = RawLexer::new(input).collect();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], Ok((0, sym("$"), 1)));
    }

    #[test]
    fn dollar_symbol_with_text() {
        let input = "$foo";
        let tokens: Vec<_> = RawLexer::new(input).collect();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0], Ok((0, sym("$foo"), 4)));
    }
}
