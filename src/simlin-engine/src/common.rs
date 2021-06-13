// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::{error, result};

use lazy_static::lazy_static;
use regex::Regex;
use std::borrow::Cow;
#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

pub type Ident = String;
pub type DimensionName = String;
pub type ElementName = String;

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    NoError,      // will never be produced
    DoesNotExist, // the named entity doesn't exist
    XmlDeserialization,
    VensimConversion,
    ProtobufDecode,
    InvalidToken,
    UnrecognizedEof,
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
    UnitDefinitionErrors,
    Generic,
    NoAppInUnits,
    NoSubscriptInUnits,
    NoIfInUnits,
    NoUnaryOpInUnits,
    BadBinaryOpInUnits,
    NoConstInUnits,
    ExpectedInteger,
    ExpectedIntegerOne,
    DuplicateUnit,
    ExpectedModule,
    ExpectedIdent,
    ZeroArityBuiltin,
}

#[cfg(not(tarpaulin_include))]
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
            UnrecognizedEof => "unrecognized_eof",
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
            UnitDefinitionErrors => "unit_definition_errors",
            Generic => "generic",
            NoAppInUnits => "no_app_in_units",
            NoSubscriptInUnits => "no_subscript_in_units",
            NoIfInUnits => "no_if_in_units",
            NoUnaryOpInUnits => "no_unary_op_in_units",
            BadBinaryOpInUnits => "bad_binary_op_in_units",
            NoConstInUnits => "no_const_in_units",
            ExpectedInteger => "expected_integer",
            ExpectedIntegerOne => "expected_integer_one",
            DuplicateUnit => "duplicate_unit",
            ExpectedModule => "expected_module",
            ExpectedIdent => "expected_ident",
            ZeroArityBuiltin => "zero_arity_builtin",
        };

        write!(f, "{}", name)
    }
}

#[cfg_attr(feature = "wasm", wasm_bindgen)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EquationError {
    pub start: u16,
    pub end: u16,
    pub code: ErrorCode,
}

impl fmt::Display for EquationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{}:{}", self.start, self.end, self.code)
    }
}

impl From<Error> for EquationError {
    fn from(err: Error) -> Self {
        EquationError {
            code: err.code,
            start: 0,
            end: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum VariableError {
    EquationError(EquationError),
    UnitError(EquationError),
}

impl fmt::Display for VariableError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            VariableError::EquationError(err) => write!(f, "eqn:{}", err),
            VariableError::UnitError(err) => write!(f, "unit:{}", err),
        }
    }
}

impl From<(Ident, EquationError)> for Error {
    fn from(err: (Ident, EquationError)) -> Self {
        Error {
            kind: ErrorKind::Variable,
            code: err.1.code,
            details: Some(err.0),
        }
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
    ($code:tt, $start:expr, $end:expr) => {{
        use crate::common::{EquationError, ErrorCode};
        Err(EquationError{ start: $start, end: $end, code: ErrorCode::$code})
    }}
);

#[macro_export]
macro_rules! var_eqn_err(
    ($ident:expr, $code:tt, $start:expr, $end:expr) => {{
        use crate::common::{EquationError, ErrorCode};
        Err(($ident, EquationError{ start: $start, end: $end, code: ErrorCode::$code}))
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
macro_rules! sim_err {
    ($code:tt, $str:expr) => {{
        use crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error {
            kind: ErrorKind::Simulation,
            code: ErrorCode::$code,
            details: Some($str),
        })
    }};
    ($code:tt) => {{
        use crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error {
            kind: ErrorKind::Simulation,
            code: ErrorCode::$code,
            details: None,
        })
    }};
}

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

#[cfg(not(tarpaulin_include))]
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

    lazy_static! {
        // TODO: \x{C2AO} ?
        static ref UNDERSCORE_RE: Regex = Regex::new(r"(\\n|\\r|\n|\r| |\x{00A0})+").unwrap();
        // parses a."b \" c" into: ('a.', '"b \" c"')
        static ref QUOTED_RE: Regex = Regex::new(r#"[^"]+|"((\\")|[^"])*""#).unwrap();
    }

    let mut canonicalized_name = String::with_capacity(name.len());

    for part in QUOTED_RE.find_iter(name).map(|part| part.as_str()) {
        let bytes = part.as_bytes();
        let quoted: bool =
            { bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' };

        let part = if quoted {
            Cow::Borrowed(&part[1..bytes.len() - 1])
        } else {
            Cow::Owned(part.replace(".", "·"))
        };

        let part = part.replace("\\\\", "\\");
        let part = UNDERSCORE_RE.replace_all(&part, "_");
        let part = part.to_lowercase();

        canonicalized_name.push_str(&part);
    }

    canonicalized_name
}

#[test]
fn test_canonicalize() {
    assert_eq!("a.b", canonicalize("\"a.b\""));
    assert_eq!("a/d·b_\\\"c\\\"", canonicalize("\"a/d\".\"b \\\"c\\\"\""));
    assert_eq!("a/d·b_c", canonicalize("\"a/d\".\"b c\""));
    assert_eq!("a·b_c", canonicalize("a.\"b c\""));
    assert_eq!("a/d·b", canonicalize("\"a/d\".b"));
    assert_eq!("quoted", canonicalize("\"quoted\""));
    assert_eq!("a_b", canonicalize("   a b"));
    assert_eq!("å_b", canonicalize("Å\nb"));
    assert_eq!("a_b", canonicalize("a \n b"));
    assert_eq!("a·b", canonicalize("a.b"));
}

pub fn quoteize(ident: &str) -> String {
    // FIXME: this needs to be smarter
    ident.replace('·', ".")
}

#[test]
fn test_quoteize() {
    assert_eq!("a_b", quoteize("a_b"));
    assert_eq!("a.b", quoteize("a·b"));
}

pub fn topo_sort<'out>(
    runlist: Vec<&'out str>,
    dependencies: &'out HashMap<Ident, BTreeSet<Ident>>,
) -> Vec<&'out str> {
    use std::collections::HashSet;

    let runlist_len = runlist.len();
    let mut result: Vec<&'out str> = Vec::with_capacity(runlist_len);
    // TODO: remove this allocation (should be &str)
    let mut used: HashSet<&str> = HashSet::new();

    // We want to do a postorder, recursive traversal of variables to ensure
    // dependencies are calculated before the variables that reference them.
    // By this point, we have already errored out if we have e.g. a cycle
    fn add<'a>(
        dependencies: &'a HashMap<Ident, BTreeSet<Ident>>,
        result: &mut Vec<&'a str>,
        used: &mut HashSet<&'a str>,
        ident: &'a str,
    ) {
        if used.contains(ident) {
            return;
        }
        used.insert(ident);
        for dep in dependencies[ident].iter() {
            add(dependencies, result, used, dep)
        }
        result.push(ident);
    }

    for ident in runlist.into_iter() {
        add(dependencies, &mut result, &mut used, ident);
    }

    assert_eq!(runlist_len, result.len());
    result
}

#[inline(always)]
/// len_utf8 returns the number of bytes needed to represent a
/// unicode character as a utf8 sequence. taken from char.len_utf8,
/// but made const. (TODO: remove this when the stdlib version is const)
pub const fn len_utf8(code: char) -> usize {
    const MAX_ONE_B: u32 = 0x80;
    const MAX_TWO_B: u32 = 0x800;
    const MAX_THREE_B: u32 = 0x10000;

    let code = code as u32;
    if code < MAX_ONE_B {
        1
    } else if code < MAX_TWO_B {
        2
    } else if code < MAX_THREE_B {
        3
    } else {
        4
    }
}

#[test]
fn test_len_utf8() {
    assert_eq!(1, len_utf8('a'));
    assert_eq!(2, len_utf8('·'));
    assert_eq!(3, len_utf8('⁚'));
    assert_eq!(4, len_utf8('🁓'));
}
