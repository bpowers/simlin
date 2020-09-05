// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::convert::From;
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
    EmptyEquation,
    TODOModules,
    NotSimulatable,
    BadSimSpecs,
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
            EmptyEquation => "empty_equation",
            TODOModules => "TODO_modules",
            NotSimulatable => "not_simulatable",
            BadSimSpecs => "bad_sim_specs",
        };

        write!(f, "{}", name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableError {
    pub location: usize,
    pub code: ErrorCode,
}

impl fmt::Display for VariableError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}", self.location, self.code)
    }
}

// from https://stackoverflow.com/questions/27588416/how-to-send-output-to-stderr
#[macro_export]
macro_rules! eprintln(
    ($($arg:tt)*) => { {
        use std::io::Write;
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    } }
);

#[macro_export]
macro_rules! die(
    ($($arg:tt)*) => { {
        use std;
        eprintln!($($arg)*);
        std::process::exit(1/*EXIT_FAILURE*/)
    } }
);

#[macro_export]
macro_rules! err(
    ($($arg:tt)*) => { {
        use crate::common::SDError;
        Err(SDError::new(format!($($arg)*)))
    } }
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

macro_rules! var_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode};
        Err(Error::VariableError(ErrorCode::$code, $str))
    }}
);

macro_rules! sim_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode};
        Err(Error::SimulationError(ErrorCode::$code, $str))
    }}
);

#[derive(Debug)]
pub struct SDError {
    msg: String,
}

type ModelName = String;
type VariableName = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    // error reading XML file or hydrating xmile structures
    ImportError(ErrorCode, String),
    ModelError(ErrorCode, ModelName),
    VariableError(ErrorCode, VariableName),
    SimulationError(ErrorCode, VariableName),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::ImportError(code, msg) => write!(f, "ImportError{{{}: {}}}", msg, code),
            Error::ModelError(code, model) => write!(f, "ModelError{{{}: {}}}", model, code),
            Error::VariableError(code, var) => write!(f, "VariableError{{{}: {}}}", var, code),
            Error::SimulationError(code, var) => write!(f, "SimulationError{{{}: {}}}", var, code),
        }
    }
}

impl error::Error for Error {}

impl SDError {
    pub fn new(msg: String) -> SDError {
        SDError { msg }
    }
}

impl fmt::Display for SDError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

impl error::Error for SDError {
    fn description(&self) -> &str {
        &self.msg
    }
}

impl From<std::io::Error> for SDError {
    fn from(err: std::io::Error) -> Self {
        SDError {
            msg: format!("io::Error: {:?}", err),
        }
    }
}

impl From<core::num::ParseFloatError> for SDError {
    fn from(err: core::num::ParseFloatError) -> Self {
        SDError {
            msg: format!("ParseFloatError: {:?}", err),
        }
    }
}

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
