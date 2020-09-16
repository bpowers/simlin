// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::ErrorCode::*;
use super::Token::*;
use super::{EquationError, ErrorCode, Lexer, Token};

// straight from LALRPOP
fn test(input: &str, expected: Vec<(&str, Token)>) {
    // use $ to signal EOL because it can be replaced with a single space
    // for spans, and because it applies also to r#XXX# style strings:
    let input = input.replace("$", "\n");

    let tokenizer = Lexer::new(&input);
    let len = expected.len();
    for (token, (expected_span, expected_tok)) in tokenizer.zip(expected.into_iter()) {
        let expected_start = expected_span.find("~").unwrap();
        let expected_end = expected_span.rfind("~").unwrap() + 1;
        assert_eq!(Ok((expected_start, expected_tok, expected_end)), token);
    }

    let tokenizer = Lexer::new(&input);
    assert_eq!(None, tokenizer.skip(len).next());
}

fn test_err(input: &str, expected: (&str, ErrorCode)) {
    // use $ to signal EOL because it can be replaced with a single space
    // for spans, and because it applies also to r#XXX# style strings:
    let input = input.replace("$", "\n");

    let tokenizer = Lexer::new(&input);
    let token = tokenizer.into_iter().last().unwrap();
    let (expected_span, expected_code) = expected;
    let expected_start = expected_span.find("~").unwrap();
    let expected_err = EquationError {
        location: expected_start,
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
    test("\"oh no\"", vec![("~~~~~~~", Ident("oh no"))]);
    test("oh.no", vec![("~~~~~", Ident("oh.no"))]);
}

#[test]
fn numbers() {
    test("4.0e5", vec![("~~~~~", Num("4.0e5"))]);
    test("4.0e-5", vec![("~~~~~~", Num("4.0e-5"))]);
}

#[test]
fn unclosed_comment() {
    test_err("{comment", ("~", UnclosedComment));
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
    test_err("\"ohno", ("~", UnclosedQuotedIdent));
}
