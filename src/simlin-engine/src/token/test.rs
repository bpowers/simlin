// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::ErrorCode::*;
use super::Token::*;
use super::{EquationError, ErrorCode, Lexer, LexerType, Token};

fn test(input: &str, expected: Vec<(&str, Token)>) {
    test_inner(input, expected, LexerType::Equation)
}

// straight from LALRPOP
fn test_inner(input: &str, expected: Vec<(&str, Token)>, lexer_type: LexerType) {
    // use $ to signal EOL because it can be replaced with a single space
    // for spans, and because it applies also to r#XXX# style strings:
    // let input = input.replace("$", "\n");

    let tokenizer = Lexer::new(input, lexer_type);
    let len = expected.len();
    for (token, (expected_span, expected_tok)) in tokenizer.zip(expected.into_iter()) {
        let expected_start = expected_span.find('~').unwrap();
        let expected_end = expected_span.rfind('~').unwrap() + 1;
        assert_eq!(Ok((expected_start, expected_tok, expected_end)), token);
    }

    let tokenizer = Lexer::new(input, lexer_type);
    assert_eq!(None, tokenizer.skip(len).next());
}

fn test_err(input: &str, expected: (&str, ErrorCode)) {
    test_err_inner(input, expected, LexerType::Equation)
}

fn test_err_inner(input: &str, expected: (&str, ErrorCode), lexer_type: LexerType) {
    // use $ to signal EOL because it can be replaced with a single space
    // for spans, and because it applies also to r#XXX# style strings:
    // let input = input.replace("$", "\n");

    let tokenizer = Lexer::new(input, lexer_type);
    let token = tokenizer.into_iter().last().unwrap();
    let (expected_span, expected_code) = expected;
    let expected_start = expected_span.find('~').unwrap();
    let expected_end = expected_span.rfind('~').unwrap() + 1;
    let expected_err = EquationError {
        start: expected_start as u16,
        end: expected_end as u16,
        code: expected_code,
    };
    assert_eq!(Err(expected_err), token);
}

#[test]
fn ifstmt() {
    test(
        "if 1    then 1 else 0",
        vec![
            ("~~                   ", If),
            ("   ~                 ", Num("1")),
            ("        ~~~~         ", Then),
            ("             ~       ", Num("1")),
            ("               ~~~~  ", Else),
            ("                    ~", Num("0")),
        ],
    );
}

#[test]
fn lte() {
    test("<=", vec![("~~", Lte)]);
}

#[test]
fn gte() {
    test(">=", vec![("~~", Gte)]);
}

#[test]
fn negative_num() {
    test("-3", vec![("~ ", Minus), (" ~", Num("3"))]);
}

#[test]
fn pairs() {
    test(
        "((b) 1)",
        vec![
            ("~      ", LParen),
            (" ~     ", LParen),
            ("  ~    ", Ident("b")),
            ("   ~   ", RParen),
            ("     ~ ", Num("1")),
            ("      ~", RParen),
        ],
    );
}

#[test]
fn quoted_ident() {
    test("\"a.b\"", vec![("~~~~~", Ident("\"a.b\""))]);
}

#[test]
fn comment() {
    test(
        "a{ xx   }1",
        vec![("~         ", Ident("a")), ("         ~", Num("1"))],
    );
}

#[test]
fn idents() {
    test(
        "_3 n3_",
        vec![("~~    ", Ident("_3")), ("   ~~~", Ident("n3_"))],
    );
    test("\"oh no\"", vec![("~~~~~~~", Ident("\"oh no\""))]);
    test("oh.no", vec![("~~~~~", Ident("oh.no"))]);
}

#[test]
fn dollar_idents() {
    test_inner(
        "$oh.no",
        vec![("~~~~~~", Ident("$oh.no"))],
        LexerType::Units,
    );
    test_err("$", ("~", UnrecognizedToken));
}

#[test]
fn numbers() {
    #[rustfmt::skip]
    test("4.0e5", vec![
        ("~~~~~", Num("4.0e5")),
    ]);
    #[rustfmt::skip]
    test("4.0e-5", vec![
        ("~~~~~~", Num("4.0e-5")),
    ]);
}

#[test]
fn subscripts() {
    #[rustfmt::skip]
    test("aux[1]", vec![
        ("~~~   ", Ident("aux")),
        ("   ~  ", LBracket),
        ("    ~ ", Num("1")),
        ("     ~", RBracket),
    ]);

    #[rustfmt::skip]
    test("aux[INT(TIME MOD 5) + 1])", vec![
        ("~~~                      ", Ident("aux")),
        ("   ~                     ", LBracket),
        ("    ~~~                  ", Ident("INT")),
        ("       ~                 ", LParen),
        ("        ~~~~             ", Ident("TIME")),
        ("             ~~~         ", Mod),
        ("                 ~       ", Num("5")),
        ("                  ~      ", RParen),
        ("                    ~    ", Plus),
        ("                      ~  ", Num("1")),
        ("                       ~ ", RBracket),
        ("                        ~", RParen),
    ]);

    #[rustfmt::skip]
    test("SUM(z[*])", vec![
        ("~~~      ", Ident("SUM")),
        ("   ~     ", LParen),
        ("    ~    ", Ident("z")),
        ("     ~   ", LBracket),
        ("      ~  ", Mul),
        ("       ~ ", RBracket),
        ("        ~", RParen),
    ]);

    #[rustfmt::skip]
    test("SUM(z[*:suba])", vec![
        ("~~~           ", Ident("SUM")),
        ("   ~          ", LParen),
        ("    ~         ", Ident("z")),
        ("     ~        ", LBracket),
        ("      ~       ", Mul),
        ("       ~      ", Colon),
        ("        ~~~~  ", Ident("suba")),
        ("            ~ ", RBracket),
        ("             ~", RParen),
    ]);

    #[rustfmt::skip]
    test("SUM(z[3:4])", vec![
        ("~~~        ", Ident("SUM")),
        ("   ~       ", LParen),
        ("    ~      ", Ident("z")),
        ("     ~     ", LBracket),
        ("      ~    ", Num("3")),
        ("       ~   ", Colon),
        ("        ~  ", Num("4")),
        ("         ~ ", RBracket),
        ("          ~", RParen),
    ]);

    #[rustfmt::skip]
    test("SUM(z[y:z])", vec![
        ("~~~        ", Ident("SUM")),
        ("   ~       ", LParen),
        ("    ~      ", Ident("z")),
        ("     ~     ", LBracket),
        ("      ~    ", Ident("y")),
        ("       ~   ", Colon),
        ("        ~  ", Ident("z")),
        ("         ~ ", RBracket),
        ("          ~", RParen),
    ]);
}

#[test]
fn floats() {
    #[rustfmt::skip]
    test("2.06101e+06", vec![
        ("~~~~~~~~~~~", Num("2.06101e+06")),
    ]);
}

#[test]
fn unclosed_comment() {
    test_err("{comment", ("~~~~~~~~", UnclosedComment));
}

#[test]
fn unclosed_comment_2() {
    test_err("comment}", ("       ~", UnrecognizedToken));
}

#[test]
fn unrecognized_token() {
    test_err("a `", ("  ~", UnrecognizedToken));
}

#[test]
fn unclosed_quoted_ident() {
    test_err("\"ohno", ("~~~~~", UnclosedQuotedIdent));
}
