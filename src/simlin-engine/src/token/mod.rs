// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// derived from both the LALRPOP whitespace tokenizer, and LALRPOP's
// internal tokenizer

use std::str::CharIndices;

use lazy_static::lazy_static;
use unicode_xid::UnicodeXID;

use self::Token::*;
use crate::common::ErrorCode::*;
use crate::common::{EquationError, ErrorCode};

#[cfg(test)]
mod test;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LexerType {
    Equation,
    Units,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token<'input> {
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
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Nan,
    Ident(&'input str),
    Num(&'input str),
}

fn error<T>(code: ErrorCode, start: usize, end: usize) -> Result<T, EquationError> {
    Err(EquationError {
        start: start as u16,
        end: end as u16,
        code,
    })
}

pub type Spanned<T> = (usize, T, usize);

pub struct Lexer<'input> {
    text: &'input str,
    chars: CharIndices<'input>,
    lookahead: Option<(usize, char)>,
    is_units: bool,
}

const KEYWORDS: &[(&str, Token<'static>)] = &[
    ("if", If),
    ("then", Then),
    ("else", Else),
    ("not", Not),
    ("mod", Mod),
    ("and", And),
    ("or", Or),
    ("nan", Nan),
];

impl<'input> Lexer<'input> {
    pub fn new(input: &'input str, lexer_type: LexerType) -> Self {
        let mut t = Lexer {
            text: input,
            chars: input.char_indices(),
            lookahead: None,
            is_units: matches!(lexer_type, LexerType::Units),
        };
        t.bump();
        t
    }

    fn bump(&mut self) -> Option<(usize, char)> {
        self.bump_n(1)
    }

    fn bump_n(&mut self, n: usize) -> Option<(usize, char)> {
        assert!(n > 0);
        self.lookahead = self.chars.nth(n - 1);
        self.lookahead
    }

    fn word(&mut self, idx0: usize) -> Spanned<&'input str> {
        let is_units = self.is_units;
        match self.take_while(|c| is_identifier_continue(c, is_units)) {
            Some(end) => (idx0, &self.text[idx0..end], end),
            None => (idx0, &self.text[idx0..], self.text.len()),
        }
    }

    fn peek(&self) -> (usize, char) {
        self.lookahead.unwrap()
    }

    fn take_while<F>(&mut self, mut keep_going: F) -> Option<usize>
    where
        F: FnMut(char) -> bool,
    {
        self.take_until(|c| !keep_going(c))
    }

    fn take_until<F>(&mut self, mut terminate: F) -> Option<usize>
    where
        F: FnMut(char) -> bool,
    {
        loop {
            match self.lookahead {
                None => {
                    return None;
                }
                Some((idx1, c)) => {
                    if terminate(c) {
                        return Some(idx1);
                    } else {
                        self.bump();
                    }
                }
            }
        }
    }

    fn identifierish(&mut self, idx0: usize) -> Spanned<Token<'input>> {
        let (start, word, end) = self.word(idx0);
        let lower_word = word.to_lowercase();

        // search for a keyword first; if none are found, this is
        // either a MacroId or an Id, depending on whether there
        // is a `<` immediately afterwards
        let tok = KEYWORDS
            .iter()
            .filter(|&&(w, _)| w == lower_word)
            .map(|(_, t)| *t)
            .next()
            .unwrap_or(Ident(word));

        (start, tok, end)
    }

    fn number(&mut self, idx0: usize) -> Spanned<Token<'input>> {
        use regex::{Match, Regex};

        lazy_static! {
            static ref NUMBER_RE: Regex =
                Regex::new(r"\d*(\.\d*)?([eE][-+]?(\d*(\.\d*)?)?)?").unwrap();
        }

        let m: Match = NUMBER_RE.find(&self.text[idx0..]).unwrap();

        self.bump_n(m.end());

        let end = idx0 + m.end();
        (idx0, Num(&self.text[idx0..end]), end)
    }

    fn quoted_identifier(&mut self, idx0: usize) -> Result<Spanned<Token<'input>>, EquationError> {
        // eat the opening '"'
        self.bump();

        match self.take_until(|c| c == '"') {
            Some(idx1) => {
                // eat the trailing '"'
                self.bump();
                Ok((idx0, Ident(&self.text[idx0..idx1 + 1]), idx1 + 1))
            }
            None => error(UnclosedQuotedIdent, idx0, self.text.len()),
        }
    }

    fn comment_end(&mut self) -> Result<(), EquationError> {
        let idx0 = self.peek().0;
        match self.take_until(|c| c == '}') {
            Some(_) => {
                self.bump(); // consume
                Ok(())
            }
            None => error(UnclosedComment, idx0, self.text.len()),
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    fn consume(
        &mut self,
        i: usize,
        tok: Token<'input>,
        len: usize,
    ) -> Option<Result<Spanned<Token<'input>>, EquationError>> {
        self.bump();
        Some(Ok((i, tok, i + len)))
    }
}

impl<'input> Iterator for Lexer<'input> {
    type Item = Result<Spanned<Token<'input>>, EquationError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            return match self.lookahead {
                Some((i, '/')) => self.consume(i, Div, 1),
                Some((i, '=')) => self.consume(i, Eq, 1),
                Some((i, '^')) => self.consume(i, Exp, 1),
                Some((i, '<')) => {
                    match self.bump() {
                        Some((_, '>')) => self.consume(i, Neq, 2),
                        Some((_, '=')) => self.consume(i, Lte, 2),
                        // we've already bumped, don't consume
                        _ => Some(Ok((i, Lt, i + 1))),
                    }
                }
                Some((i, '>')) => {
                    match self.bump() {
                        Some((_, '=')) => self.consume(i, Gte, 2),
                        // we've already bumped, don't consume
                        _ => Some(Ok((i, Gt, i + 1))),
                    }
                }
                Some((i, '&')) => {
                    match self.bump() {
                        Some((_, '&')) => self.consume(i, And, 2),
                        // we've already bumped, don't consume
                        _ => Some(error(UnrecognizedToken, i, i + 2)),
                    }
                }
                Some((i, '|')) => {
                    match self.bump() {
                        Some((_, '|')) => self.consume(i, Or, 2),
                        // we've already bumped, don't consume
                        _ => Some(error(UnrecognizedToken, i, i + 2)),
                    }
                }
                Some((i, '-')) => self.consume(i, Minus, 1),
                Some((i, '+')) => self.consume(i, Plus, 1),
                Some((i, '*')) => self.consume(i, Mul, 1),
                Some((i, ':')) => self.consume(i, Colon, 1),
                Some((i, '{')) => match self.comment_end() {
                    Ok(()) => self.next(),
                    Err(_) => Some(error(UnclosedComment, i, self.text.len())),
                },
                Some((i, '(')) => self.consume(i, LParen, 1),
                Some((i, ')')) => self.consume(i, RParen, 1),
                Some((i, '[')) => self.consume(i, LBracket, 1),
                Some((i, ']')) => self.consume(i, RBracket, 1),
                Some((i, ',')) => self.consume(i, Comma, 1),
                Some((i, '"')) => Some(self.quoted_identifier(i)),
                Some((i, c)) if is_identifier_start(c, self.is_units) => {
                    Some(Ok(self.identifierish(i)))
                }
                Some((i, c)) if is_number_start(c) => Some(Ok(self.number(i))),
                Some((_, c)) if c.is_whitespace() => {
                    self.bump();
                    continue;
                }
                Some((i, _)) => {
                    self.bump(); // eat whatever is killing us
                    let end = match self.lookahead {
                        Some((end, _)) => end,
                        None => self.text.len(),
                    };
                    Some(error(UnrecognizedToken, i, end))
                }
                None => None,
            };
        }
    }
}

fn is_number_start(c: char) -> bool {
    is_digit(c) || c == '.'
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}

fn is_identifier_start(c: char, is_units: bool) -> bool {
    UnicodeXID::is_xid_start(c) || c == '_' || (is_units && c == '$')
}

fn is_identifier_continue(c: char, is_units: bool) -> bool {
    UnicodeXID::is_xid_continue(c) || c == '.' || (is_units && c == '$')
}
