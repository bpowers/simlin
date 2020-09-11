// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fmt;
use std::{error, result};

use regex::Regex;

pub type Ident = String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    DoesNotExist, // the named entity doesn't exist
    XmlDeserialization,
    InvalidToken,
    UnrecognizedEOF,
    UnrecognizedToken,
    ExtraToken,
    UnclosedComment,
    UnclosedQuotedIdent,
    ExpectedNumber,
    UnknownBuiltin,
    BadBuiltinArgs,
    EmptyEquation,
    TODOModules,
    NotSimulatable,
    BadTable,
    BadSimSpecs,
    NoAbsoluteReferences,
    CircularDependency,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ErrorCode::*;
        let name = match self {
            DoesNotExist => "does_not_exist",
            XmlDeserialization => "xml_deserialization",
            InvalidToken => "invalid_token",
            UnrecognizedEOF => "unrecognized_eof",
            UnrecognizedToken => "unrecognized_token",
            ExtraToken => "extra_token",
            UnclosedComment => "unclosed_comment",
            UnclosedQuotedIdent => "unclosed_quoted_ident",
            ExpectedNumber => "expected_number",
            UnknownBuiltin => "unknown_builtin",
            BadBuiltinArgs => "bad_builtin_args",
            EmptyEquation => "empty_equation",
            TODOModules => "TODO_modules",
            NotSimulatable => "not_simulatable",
            BadTable => "bad_table",
            BadSimSpecs => "bad_sim_specs",
            NoAbsoluteReferences => "no_absolute_references",
            CircularDependency => "circular_dependency",
        };

        write!(f, "{}", name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquationError {
    pub location: usize,
    pub code: ErrorCode,
}

impl fmt::Display for EquationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.location, self.code)
    }
}

// from https://stackoverflow.com/questions/27588416/how-to-send-output-to-stderr
macro_rules! eprintln(
    ($($arg:tt)*) => {{
        use std::io::Write;
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    }}
);

macro_rules! import_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode};
        Err(Error::ImportError(ErrorCode::$code, $str))
    }}
);

macro_rules! model_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode};
        Err(Error::ModelError(ErrorCode::$code, $str))
    }}
);

// macro_rules! var_err(
//     ($code:tt, $str:expr) => {{
//         use crate::common::{Error, ErrorCode};
//         Err(Error::VariableError(ErrorCode::$code, $str, None))
//     }};
//     ($code:tt, $str:expr, $loc:expr) => {{
//         use crate::common::{Error, ErrorCode};
//         Err(Error::VariableError(ErrorCode::$code, $str, Some($loc)))
//     }};
// );

macro_rules! sim_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode};
        Err(Error::SimulationError(ErrorCode::$code, $str))
    }}
);

type ModelName = String;
type VariableName = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    // error reading XML file or hydrating xmile structures
    ImportError(ErrorCode, String),
    ModelError(ErrorCode, ModelName),
    VariableError(ErrorCode, VariableName, Option<usize>),
    SimulationError(ErrorCode, VariableName),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::ImportError(code, msg) => write!(f, "ImportError{{{}: {}}}", msg, code),
            Error::ModelError(code, model) => write!(f, "ModelError{{{}: {}}}", model, code),
            Error::VariableError(code, var, pos) => {
                write!(f, "VariableError{{{}:{}: {}}}", var, pos.unwrap_or(0), code)
            }
            Error::SimulationError(code, var) => write!(f, "SimulationError{{{}: {}}}", var, code),
        }
    }
}

impl error::Error for Error {}

pub type Result<T> = result::Result<T, Error>;

pub fn canonicalize(name: &str) -> String {
    // remove leading and trailing whitespace, do this before testing
    // for quotedness as we should treat a quoted string as sacrosanct
    let name = name.trim();

    let bytes = name.as_bytes();
    let quoted: bool = { bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' };

    let name = if quoted {
        &name[1..bytes.len() - 1]
    } else {
        name
    };

    lazy_static! {
        // TODO: \x{C2AO} ?
        static ref UNDERSCORE_RE: Regex = Regex::new(r"\\n|\\r|\n|\r| |\x{00A0}").unwrap();
    }
    let name = name.replace("\\\\", "\\");
    let name = UNDERSCORE_RE.replace_all(&name, "_");

    name.to_lowercase()
}

#[test]
fn test_canonicalize() {
    assert!(canonicalize("\"quoted\"") == "quoted");
    assert!(canonicalize("   a b") == "a_b");
    assert!(canonicalize("Å\nb") == "å_b");
}
