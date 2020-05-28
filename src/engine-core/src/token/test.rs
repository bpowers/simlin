// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::Token::*;
use super::{Token, Tokenizer};

// straight from LALRPOP
fn test(input: &str, expected: Vec<(&str, Token)>) {
    // use $ to signal EOL because it can be replaced with a single space
    // for spans, and because it applies also to r#XXX# style strings:
    let input = input.replace("$", "\n");

    let tokenizer = Tokenizer::new(&input);
    let len = expected.len();
    for (token, (expected_span, expected_tok)) in tokenizer.zip(expected.into_iter()) {
        println!("token: {:?}", token);
        let expected_start = expected_span.find("~").unwrap();
        let expected_end = expected_span.rfind("~").unwrap() + 1;
        assert_eq!(Ok((expected_start, expected_tok, expected_end)), token);
    }

    let tokenizer = Tokenizer::new(&input);
    assert_eq!(None, tokenizer.skip(len).next());
}

#[test]
fn ifstmt() {
    test(
        "if 1    then 1 else 0",
        vec![
            ("~~                   ", If),
            ("   ~                 ", Num(1)),
            ("        ~~~~         ", Then),
            ("             ~       ", Num(1)),
            ("               ~~~~  ", Else),
            ("                    ~", Num(0)),
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
    test("-3", vec![("~ ", Minus), (" ~", Num(3))]);
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
            ("     ~ ", Num(1)),
            ("      ~", RParen),
        ],
    );
}

#[test]
fn comment() {
    test(
        "a{ xx   }1",
        vec![("~         ", Ident("a")), ("         ~", Num(1))],
    );
}

#[test]
fn idents() {
    test(
        "_3 n3_",
        vec![("~~    ", Ident("_3")), ("   ~~~", Ident("n3_"))],
    );
}
