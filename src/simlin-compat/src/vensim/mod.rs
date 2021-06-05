// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::io::BufRead;

use simlin_engine::datamodel::Project;
use simlin_engine::Result;

mod file;
mod parser {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/vensim/parser.rs"));
}
mod token;

#[test]
fn test_vensim_parse() {
    let case = "{UTF-8}\n\
        :MACRO: EXPRESSION MACRO(input, parameter)\n\
        EXPRESSION MACRO = input * parameter\n\
                ~       input\n\
                ~       tests basic macro containing no stocks and having no output\n\
                |
        \n\
        :END OF MACRO:";
    let expected = file::File {
        macros: vec![file::Macro {
            name: "EXPRESSION MACRO".to_owned(),
            inputs: vec!["input".to_owned(), "parameter".to_owned()],
            outputs: vec![],
            variables: vec![file::Variable {
                name: "EXPRESSION MACRO".to_owned(),
                equation: "input * parameter".to_owned(),
                units: "input".to_owned(),
                comment: "tests basic macro containing no stocks and having no output".to_owned(),
                range: None,
            }],
        }],
        variables: vec![],
        control: vec![],
    };

    let lexer = token::Lexer::new(case);
    let file = parser::FileParser::new().parse(case, lexer).unwrap();
    assert_eq!(expected, file);
}

pub fn project_from_reader(reader: &mut dyn BufRead) -> Result<Project> {
    use simlin_engine::common::{Error, ErrorCode, ErrorKind};

    let mut contents_buf: Vec<u8> = vec![];
    reader
        .read_until(0, &mut contents_buf)
        .map_err(|_err| Error::new(ErrorKind::Import, ErrorCode::VensimConversion, None))?;
    let contents: String = String::from_utf8(contents_buf).unwrap();

    let lexer = token::Lexer::new(&contents);
    let file = parser::FileParser::new().parse(&contents, lexer).unwrap();
    eprintln!("FILE: {:?}\n~~~~~~", file);

    return Err(Error::new(
        ErrorKind::Import,
        ErrorCode::VensimConversion,
        Some("unknown xmutil error".to_owned()),
    ));
}
