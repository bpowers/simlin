// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fmt;
use std::{error, result};

#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

use lazy_static::lazy_static;
use regex::Regex;

pub type Ident = String;
pub type DimensionName = String;
pub type ElementName = String;

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    NoError,      // will never be produced
    DoesNotExist, // the named entity doesn't exist
    XmlDeserialization,
    VensimConversion,
    ProtobufDecode,
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
    BadModuleInputDst,
    BadModuleInputSrc,
    NotSimulatable,
    BadTable,
    BadSimSpecs,
    NoAbsoluteReferences,
    CircularDependency,
    ArraysNotImplemented,
    MultiDimensionalArraysNotImplemented,
    BadDimensionName,
    BadModelName,
    MismatchedDimensions,
    ArrayReferenceNeedsExplicitSubscripts,
    DuplicateVariable,
    UnknownDependency,
    VariablesHaveErrors,
    Generic,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ErrorCode::*;
        let name = match self {
            NoError => "no_error",
            DoesNotExist => "does_not_exist",
            XmlDeserialization => "xml_deserialization",
            VensimConversion => "vensim_conversion",
            ProtobufDecode => "protobuf_decode",
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
            BadModuleInputSrc => "bad_module_input_src",
            BadModuleInputDst => "bad_module_input_dst",
            NotSimulatable => "not_simulatable",
            BadTable => "bad_table",
            BadSimSpecs => "bad_sim_specs",
            NoAbsoluteReferences => "no_absolute_references",
            CircularDependency => "circular_dependency",
            ArraysNotImplemented => "arrays_not_implemented",
            MultiDimensionalArraysNotImplemented => "multi_dimensional_arrays_not_implemented",
            BadDimensionName => "bad_dimension_name",
            BadModelName => "bad_model_name",
            MismatchedDimensions => "mismatched_dimensions",
            ArrayReferenceNeedsExplicitSubscripts => "array_reference_needs_explicit_subscripts",
            DuplicateVariable => "duplicate_variable",
            UnknownDependency => "unknown_dependency",
            VariablesHaveErrors => "variables_have_errors",
            Generic => "generic",
        };

        write!(f, "{}", name)
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen)]
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
#[macro_export]
macro_rules! eprintln(
    ($($arg:tt)*) => {{
        use std::io::Write;
        let r = writeln!(&mut ::std::io::stderr(), $($arg)*);
        r.expect("failed printing to stderr");
    }}
);

#[macro_export]
macro_rules! eqn_err(
    ($code:tt, $off:expr) => {{
        use crate::common::ErrorCode;
        Err(EquationError{ location: $off, code: ErrorCode::$code})
    }}
);

#[macro_export]
macro_rules! model_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error{
            kind: ErrorKind::Model,
            code: ErrorCode::$code,
            details: Some($str),
        })
    }}
);

#[macro_export]
macro_rules! var_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{EquationError, ErrorCode};
        Err(EquationError{
            code: ErrorCode::$code,
            location: 0,
        })
    }}
);

#[macro_export]
macro_rules! sim_err(
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error{
            kind: ErrorKind::Simulation,
            code: ErrorCode::$code,
            details: Some($str),
        })
    }}
);

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Import,
    Model,
    Simulation,
    Variable,
}

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub code: ErrorCode,
    pub(crate) details: Option<String>,
}

impl From<Box<dyn std::error::Error>> for Error {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        Error {
            kind: ErrorKind::Simulation,
            code: ErrorCode::Generic,
            details: Some(err.to_string()),
        }
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen)]
impl Error {
    pub fn new(kind: ErrorKind, code: ErrorCode, details: Option<String>) -> Self {
        Error {
            kind,
            code,
            details,
        }
    }

    #[cfg_attr(feature = "wasm", wasm_bindgen(js_name = getDetails))]
    pub fn get_details(&self) -> Option<String> {
        self.details.clone()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let kind = match self.kind {
            ErrorKind::Import => "ImportError",
            ErrorKind::Model => "ModelError",
            ErrorKind::Simulation => "SimulationError",
            ErrorKind::Variable => "VariableError",
        };
        match self.details {
            Some(ref details) => write!(f, "{}{{{}: {}}}", kind, self.code, details),
            None => write!(f, "{}{{{}}}", kind, self.code),
        }
    }
}

impl error::Error for Error {}

pub type Result<T> = result::Result<T, Error>;
pub type EquationResult<T> = result::Result<T, EquationError>;

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
