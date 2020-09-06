// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// derived from both the LALRPOP whitespace tokenizer, and LALRPOP's
// internal tokenizer

use std::str::CharIndices;
use unicode_xid::UnicodeXID;

use self::Token::*;
use crate::common::ErrorCode::*;
use crate::common::{EquationError, ErrorCode};

#[cfg(test)]
mod test;

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
    Ident(&'input str),
    Num(&'input str),
}

fn error<T>(c: ErrorCode, l: usize) -> Result<T, EquationError> {
    Err(EquationError {
        location: l,
        code: c,
    })
}

pub type Spanned<T> = (usize, T, usize);

pub struct Lexer<'input> {
    text: &'input str,
    chars: CharIndices<'input>,
    lookahead: Option<(usize, char)>,
}

const KEYWORDS: &[(&str, Token<'static>)] = &[
    ("if", If),
    ("then", Then),
    ("else", Else),
    ("not", Not),
    ("mod", Mod),
    ("and", And),
    ("or", Or),
];

impl<'input> Lexer<'input> {
    pub fn new(input: &'input str) -> Self {
        let mut t = Lexer {
            text: input,
            chars: input.char_indices(),
            lookahead: None,
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
        match self.take_while(is_identifier_continue) {
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

    fn identifierish(&mut self, idx0: usize) -> Result<Spanned<Token<'input>>, EquationError> {
        let (start, word, end) = self.word(idx0);
        let lower_word = word.to_lowercase();

        // search for a keyword first; if none are found, this is
        // either a MacroId or an Id, depending on whether there
        // is a `<` immediately afterwards
        let tok = KEYWORDS
            .iter()
            .filter(|&&(w, _)| w == lower_word)
            .map(|&(_, ref t)| *t)
            .next()
            .unwrap_or_else(|| Ident(word));

        Ok((start, tok, end))
    }

    fn number(&mut self, idx0: usize) -> Result<Spanned<Token<'input>>, EquationError> {
        use regex::{Match, Regex};

        lazy_static! {
            static ref NUMBER_RE: Regex =
                Regex::new(r"\d*(\.\d*)?([eE]-?(\d?(\.\d*)?)?)?").unwrap();
        }

        let m: Match = NUMBER_RE.find(&self.text[idx0..]).unwrap();
        assert_eq!(m.start(), 0);

        self.bump_n(m.end());

        let end = idx0 + m.end();
        Ok((idx0, Num(&self.text[idx0..end]), end))
    }

    fn quoted_identifier(&mut self, idx0: usize) -> Result<Spanned<Token<'input>>, EquationError> {
        // eat the opening '"'
        self.bump();

        match self.take_until(|c| c == '"') {
            Some(idx1) => {
                // eat the trailing '"'
                self.bump();
                Ok((idx0, Ident(&self.text[idx0 + 1..idx1]), idx1 + 1))
            }
            None => Err(EquationError {
                location: idx0,
                code: UnclosedQuotedIdent,
            }),
        }
    }

    fn comment_end(&mut self) -> Result<(), EquationError> {
        let idx0 = self.peek().0;
        match self.take_until(|c| c == '}') {
            Some(_) => {
                self.bump(); // consume
                Ok(())
            }
            None => error(UnclosedComment, idx0),
        }
    }
}
impl<'input> Iterator for Lexer<'input> {
    type Item = Result<Spanned<Token<'input>>, EquationError>;

    fn next(&mut self) -> Option<Self::Item> {
        macro_rules! consume {
            ($s: expr, $i:expr, $tok:expr, $len:expr) => {{
                $s.bump();
                Some(Ok(($i, $tok, $i + $len)))
            }};
        }

        loop {
            return match self.lookahead {
                Some((i, '/')) => consume!(self, i, Div, 1),
                Some((i, '=')) => consume!(self, i, Eq, 1),
                Some((i, '^')) => consume!(self, i, Exp, 1),
                Some((i, '<')) => {
                    match self.bump() {
                        Some((_, '>')) => consume!(self, i, Neq, 2),
                        Some((_, '=')) => consume!(self, i, Lte, 2),
                        _ => {
                            // we've already bumped, don't consume
                            Some(Ok((i, Lt, i + 1)))
                        }
                    }
                }
                Some((i, '>')) => {
                    match self.bump() {
                        Some((_, '=')) => consume!(self, i, Gte, 2),
                        _ => {
                            // we've already bumped, don't consume
                            Some(Ok((i, Gt, i + 1)))
                        }
                    }
                }
                Some((i, '-')) => consume!(self, i, Minus, 1),
                Some((i, '+')) => consume!(self, i, Plus, 1),
                Some((i, '*')) => consume!(self, i, Mul, 1),
                Some((i, '{')) => match self.comment_end() {
                    Ok(()) => self.next(),
                    Err(_) => Some(error(UnclosedComment, i)),
                },
                Some((i, '(')) => consume!(self, i, LParen, 1),
                Some((i, ')')) => consume!(self, i, RParen, 1),
                Some((i, '[')) => consume!(self, i, LBracket, 1),
                Some((i, ']')) => consume!(self, i, RBracket, 1),
                Some((i, ',')) => consume!(self, i, Comma, 1),
                Some((i, '"')) => Some(self.quoted_identifier(i)),
                Some((i, c)) if is_identifier_start(c) => Some(self.identifierish(i)),
                Some((i, c)) if is_number_start(c) => Some(self.number(i)),
                Some((_, c)) if c.is_whitespace() => {
                    self.bump();
                    continue;
                }
                Some((i, _)) => Some(error(UnrecognizedToken, i)),
                None => None,
            };
        }
    }
}

fn is_number_start(c: char) -> bool {
    is_digit(c) || c == '.'
}

fn is_digit(c: char) -> bool {
    '9' >= c && c >= '0'
}

fn is_identifier_start(c: char) -> bool {
    UnicodeXID::is_xid_start(c) || c == '_'
}

fn is_identifier_continue(c: char) -> bool {
    UnicodeXID::is_xid_continue(c) || c == '.'
}
