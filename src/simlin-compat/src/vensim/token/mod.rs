// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// derived from the LALRPOP whitespace tokenizer, LALRPOP's
// internal tokenizer, and xmutil's VensimLex tokenizer

use std::str::CharIndices;

use lazy_static::lazy_static;
use unicode_xid::UnicodeXID;

use self::Token::*;
use simlin_engine::common::ErrorCode::*;
use simlin_engine::common::{EquationError, ErrorCode};

#[cfg(test)]
mod test;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token<'input> {
    DataEquals,
    WithLookup,
    Map,
    Equiv,
    GroupStar,
    MacroStart,
    MacroEnd,
    HoldBackward,
    LookForward,
    Except,
    NA,
    Interpolate,
    Raw,
    TestInput,
    TheCondition,
    Implies,
    TabbedArray,
    EqEnd,
    EqSeparator,
    Units,
    EndUnits,

    And,
    Or,
    Not,

    Gte,
    Lte,
    Neq,

    Eq,
    Exp,
    Lt,
    Gt,
    Plus,
    Minus,
    Mul,
    Div,

    LParen,
    RParen,
    LBracket,
    RBracket,

    Comma,

    Ident(&'input str),
    Num(&'input str),
    Symbol(&'input str),
    UnitsSymbol(&'input str),
    Function(&'input str),
}

fn inner_error(code: ErrorCode, start: usize, end: usize) -> EquationError {
    EquationError {
        start: start as u16,
        end: end as u16,
        code,
    }
}

fn error<T>(code: ErrorCode, start: usize, end: usize) -> Result<T, EquationError> {
    Err(inner_error(code, start, end))
}

pub type Spanned<T> = (usize, T, usize);

pub struct Lexer<'input> {
    text: &'input str,
    chars: CharIndices<'input>,
    lookahead: Option<(usize, char)>,
}

const SYMBOLS: &[(&str, Token<'static>)] = &[
    ("MACRO", MacroStart),
    ("END OF MACRO", MacroEnd),
    ("AND", And),
    ("OR", Or),
    ("NOT", Not),
    ("NA", NA),
    ("EXCEPT", Except),
    ("HOLD BACKWARD", HoldBackward),
    ("IMPLIES", Implies),
    ("INTERPOLATE", Interpolate),
    ("LOOK FORWARD", LookForward),
    ("RAW", Raw),
    ("TEST INPUT", TestInput),
    ("THE CONDITION", TheCondition),
];

impl<'input> Lexer<'input> {
    pub fn new(input: &'input str) -> Self {
        let mut t = Lexer {
            text: input,
            chars: input.char_indices(),
            lookahead: None,
        };

        let n = {
            use regex::Regex;

            lazy_static! {
                static ref UTF8_RE: Regex = Regex::new(r"\s*(\{(UTF|utf)-8\})?").unwrap();
            }

            match UTF8_RE.find(t.text) {
                Some(m) => m.end() + 1,
                None => 1,
            }
        };

        t.bump_n(n);
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

    fn peek(&self) -> (usize, char) {
        self.lookahead.unwrap()
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

    fn symbol(&mut self, idx0: usize) -> Result<Spanned<Token<'input>>, EquationError> {
        self.bump(); // skip past the ':' at idx0
        let idx1 =
            self.take_until(|c| c == ':')
                .ok_or(inner_error(UnclosedSymbol, idx0, idx0 + 1))?;
        self.bump(); // skip past the ':' at idx1

        let symbol = &self.text[idx0 + 1..idx1];

        let tok = SYMBOLS
            .iter()
            .filter(|&&(w, _)| w == symbol)
            .map(|&(_, ref t)| *t)
            .next()
            .ok_or(inner_error(UnknownSymbol, idx0, idx1))?;

        Ok((idx0, tok, idx1 + 1))
    }

    fn identifier(&mut self, idx0: usize) -> Spanned<Token<'input>> {
        use regex::{Match, Regex};

        lazy_static! {
            // we can have internal spaces, but not trailing spaces
            static ref IDENT_RE: Regex = Regex::new(r"\w*(\w|\d|\s|_|$|')*(\w|\d|_|$|')").unwrap();
        }

        let m: Match = IDENT_RE.find(&self.text[idx0..]).unwrap();

        self.bump_n(m.end());

        let end = idx0 + m.end();
        (idx0, Ident(&self.text[idx0..end]), end)
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

    fn group_name(&mut self, idx0: usize) -> Result<Spanned<Token<'input>>, EquationError> {
        use regex::{Match, Regex};

        lazy_static! {
            static ref STAR_RE: Regex = Regex::new(r"\*\*+").unwrap();
        }

        let m: Match = STAR_RE.find(&self.text[idx0..]).unwrap();
        self.bump_n(m.end());

        self.next()
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
                Some((i, '|')) => match self.bump() {
                    Some((_, '|')) => self.consume(i, Or, 2),
                    // we've already bumped, don't consume
                    _ => Some(Ok((i, EqEnd, i + 1))),
                },
                Some((i, '-')) => self.consume(i, Minus, 1),
                Some((i, '+')) => self.consume(i, Plus, 1),
                Some((i, '*')) => {
                    match self.bump() {
                        Some((_, '*')) => self.group_name(i),
                        // we've already bumped, don't consume
                        _ => Some(Ok((i, Mul, i + 1))),
                    }
                }
                Some((i, '{')) => match self.comment_end() {
                    Ok(()) => self.next(),
                    Err(_) => Some(error(UnclosedComment, i, self.text.len())),
                },
                Some((i, '(')) => self.consume(i, LParen, 1),
                Some((i, ')')) => self.consume(i, RParen, 1),
                Some((i, '[')) => self.consume(i, LBracket, 1),
                Some((i, ']')) => self.consume(i, RBracket, 1),
                Some((i, ',')) => self.consume(i, Comma, 1),
                Some((i, '~')) => self.consume(i, EqSeparator, 1),
                Some((i, '"')) => Some(self.quoted_identifier(i)),
                Some((i, c)) if is_identifier_start(c) => Some(Ok(self.identifier(i))),
                Some((i, c)) if is_number_start(c) => Some(Ok(self.number(i))),
                Some((_, c)) if c.is_whitespace() => {
                    self.bump();
                    continue;
                }
                Some((i, ':')) => Some(self.symbol(i)),
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
    ('0'..='9').contains(&c)
}

fn is_identifier_start(c: char) -> bool {
    UnicodeXID::is_xid_start(c) || c == '_'
}
