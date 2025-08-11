// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// FIXME: remove when wasm-bindgen is updated past 0.2.79
#![allow(clippy::unused_unit)]

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::{error, result};

use crate::ast::Loc;
use lazy_static::lazy_static;
use regex::Regex;
use std::borrow::Cow;
#[cfg(feature = "wasm")]
use wasm_bindgen::prelude::*;

// Legacy type aliases - to be deprecated
pub type DimensionName = String;
pub type ElementName = String;

/// A canonicalized identifier - guaranteed to be in canonical form
///
/// Canonical form means:
/// - Lowercase
/// - Spaces/newlines replaced with underscores
/// - Dots outside quotes replaced with middle dot (·)
/// - Properly handles quoted sections
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalIdent(String);

/// A raw, non-canonicalized identifier as it appears in source
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RawIdent(String);

/// A canonicalized dimension name
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalDimensionName(String);

/// A raw dimension name as it appears in source
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RawDimensionName(String);

/// A canonicalized element name (dimension element)
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalElementName(String);

/// A raw element name as it appears in source
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RawElementName(String);

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
    UnitMismatch,
    TodoWildcard,
    TodoStarRange,
    TodoRange,
    TodoArrayBuiltin,
    CantSubscriptScalar,
    DimensionInScalarContext,
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
            UnitMismatch => "unit_mismatch",
            TodoWildcard => "todo_wildcard",
            TodoStarRange => "todo_star_range",
            TodoRange => "todo_range",
            TodoArrayBuiltin => "todo_array_builtin",
            CantSubscriptScalar => "cant_subscript_scalar",
            DimensionInScalarContext => "dimension_in_scalar_context",
        };

        write!(f, "{name}")
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
pub enum UnitError {
    DefinitionError(EquationError, Option<String>),
    ConsistencyError(ErrorCode, Loc, Option<String>),
}

impl fmt::Display for UnitError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            UnitError::DefinitionError(err, details) => {
                if let Some(details) = details {
                    write!(f, "unit definition:{err} -- {details}")
                } else {
                    write!(f, "unit definition:{err}")
                }
            }
            UnitError::ConsistencyError(err, loc, details) => {
                if let Some(details) = details {
                    write!(f, "unit consistency:{loc}:{err} -- {details}")
                } else {
                    write!(f, "unit consistency:{loc}:{err}")
                }
            }
        }
    }
}

impl From<(CanonicalIdent, EquationError)> for Error {
    fn from(err: (CanonicalIdent, EquationError)) -> Self {
        Error {
            kind: ErrorKind::Variable,
            code: err.1.code,
            details: Some(err.0.as_str().to_owned()),
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
        use $crate::common::{EquationError, ErrorCode};
        Err(EquationError{ start: $start, end: $end, code: ErrorCode::$code})
    }}
);

#[macro_export]
macro_rules! var_eqn_err(
    ($ident:expr, $code:tt, $start:expr, $end:expr) => {{
        use $crate::common::{EquationError, ErrorCode};
        Err(($ident, EquationError{ start: $start, end: $end, code: ErrorCode::$code}))
    }}
);

#[macro_export]
macro_rules! model_err(
    ($code:tt, $str:expr) => {{
        use $crate::common::{Error, ErrorCode, ErrorKind};
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
        use $crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error {
            kind: ErrorKind::Simulation,
            code: ErrorCode::$code,
            details: Some($str),
        })
    }};
    ($code:tt) => {{
        use $crate::common::{Error, ErrorCode, ErrorKind};
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
pub type UnitResult<T> = result::Result<T, UnitError>;

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
            Cow::Owned(part.replace('.', "·"))
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

#[test]
fn test_canonical_ident() {
    // Test canonicalization from raw
    let raw = RawIdent::new("Hello World".to_string());
    let canonical = raw.canonicalize();
    assert_eq!(canonical.as_str(), "hello_world");

    // Test direct creation
    let canonical2 = CanonicalIdent::from_raw("Hello World");
    assert_eq!(canonical, canonical2);

    // Test quoteize
    let canonical3 = CanonicalIdent::from_raw("a.b");
    assert_eq!(canonical3.as_str(), "a·b");
    assert_eq!(canonical3.quoteize(), "a.b");

    // Test conversion to String (using Display trait)
    let legacy: String = canonical.to_string();
    assert_eq!(legacy, "hello_world");
}

#[test]
fn test_canonical_dimension_name() {
    let raw = RawDimensionName::new("Time Units".to_string());
    let canonical = raw.canonicalize();
    assert_eq!(canonical.as_str(), "time_units");

    let canonical2 = CanonicalDimensionName::from_raw("Time Units");
    assert_eq!(canonical, canonical2);
}

#[test]
fn test_canonical_element_name() {
    let raw = RawElementName::new("Element Name".to_string());
    let canonical = raw.canonicalize();
    assert_eq!(canonical.as_str(), "element_name");

    let canonical2 = CanonicalElementName::from_raw("Element Name");
    assert_eq!(canonical, canonical2);
}

#[test]
fn test_canonical_ident_with_dots() {
    // Test that dots outside quotes become middle dots
    let c1 = CanonicalIdent::from_raw("a.d");
    assert_eq!(c1.as_str(), "a·d");

    // Test that quoted identifiers with dots keep them as middle dots after canonicalization
    let c2 = CanonicalIdent::from_raw("\"a.d\"");
    assert_eq!(c2.as_str(), "a.d");

    // Test mixed case
    let c3 = CanonicalIdent::from_raw("a.\"b.c\"");
    assert_eq!(c3.as_str(), "a·b.c");
}

#[test]
fn test_stdlib_model_name_canonicalization() {
    // Test canonicalization of stdlib model names
    let stdlib_name = "stdlib⁚smth1";
    let canonical = CanonicalIdent::from_raw(stdlib_name);
    assert_eq!(canonical.as_str(), "stdlib⁚smth1");

    // Test that the Display trait's to_string() preserves the name
    assert_eq!(canonical.to_string(), "stdlib⁚smth1");
}

#[test]
fn test_stdlib_variable_canonicalization() {
    // Test that stdlib variable names are canonicalized correctly
    let names = vec!["input", "output", "Output", "delay_time", "initial_value"];
    for name in names {
        let canonical = CanonicalIdent::from_raw(name);
        let expected = canonicalize(name);
        assert_eq!(canonical.as_str(), expected, "Failed for {}", name);
    }

    // Specifically test Output -> output conversion
    assert_eq!(CanonicalIdent::from_raw("Output").as_str(), "output");
}

// Implementations for identifier types

impl CanonicalIdent {
    /// Create from an already-canonicalized string (internal use only)
    ///
    /// # Safety
    /// Caller must guarantee the string is already in canonical form
    #[allow(dead_code)]
    pub fn from_canonical_unchecked(s: String) -> Self {
        CanonicalIdent(s)
    }

    /// Create from an already-canonicalized string (internal use only)
    ///
    /// # Safety
    /// Caller must guarantee the string is already in canonical form
    #[allow(dead_code)]
    pub fn from_canonical_str_unchecked(s: &str) -> Self {
        CanonicalIdent(s.to_string())
    }

    /// Create from a raw string, canonicalizing it
    pub fn from_raw(s: &str) -> Self {
        CanonicalIdent(canonicalize(s))
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Get a quoteized version for display
    pub fn quoteize(&self) -> String {
        quoteize(&self.0)
    }
}

impl RawIdent {
    /// Create a new raw identifier
    pub fn new(s: String) -> Self {
        RawIdent(s)
    }

    /// Create from a string slice
    pub fn new_from_str(s: &str) -> Self {
        RawIdent(s.to_string())
    }

    /// Canonicalize this identifier
    pub fn canonicalize(&self) -> CanonicalIdent {
        CanonicalIdent(canonicalize(&self.0))
    }

    /// Get the underlying raw string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CanonicalDimensionName {
    /// Create from an already-canonicalized string (internal use only)
    #[allow(dead_code)]
    pub(crate) fn from_canonical_unchecked(s: String) -> Self {
        CanonicalDimensionName(s)
    }

    /// Create from a raw string, canonicalizing it
    pub fn from_raw(s: &str) -> Self {
        CanonicalDimensionName(canonicalize(s))
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert to the legacy DimensionName type (for gradual migration)
    pub fn to_dimension_name(&self) -> DimensionName {
        self.0.clone()
    }
}

impl RawDimensionName {
    /// Create a new raw dimension name
    pub fn new(s: String) -> Self {
        RawDimensionName(s)
    }

    /// Canonicalize this dimension name
    pub fn canonicalize(&self) -> CanonicalDimensionName {
        CanonicalDimensionName(canonicalize(&self.0))
    }

    /// Get the underlying raw string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CanonicalElementName {
    /// Create from an already-canonicalized string (internal use only)
    #[allow(dead_code)]
    pub(crate) fn from_canonical_unchecked(s: String) -> Self {
        CanonicalElementName(s)
    }

    /// Create from a raw string, canonicalizing it
    pub fn from_raw(s: &str) -> Self {
        CanonicalElementName(canonicalize(s))
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert to the legacy ElementName type (for gradual migration)
    pub fn to_element_name(&self) -> ElementName {
        self.0.clone()
    }
}

impl RawElementName {
    /// Create a new raw element name
    pub fn new(s: String) -> Self {
        RawElementName(s)
    }

    /// Canonicalize this element name
    pub fn canonicalize(&self) -> CanonicalElementName {
        CanonicalElementName(canonicalize(&self.0))
    }

    /// Get the underlying raw string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Display implementations for better debugging
impl fmt::Display for CanonicalIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for RawIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for CanonicalDimensionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for RawDimensionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for CanonicalElementName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for RawElementName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// Conversion to String
impl From<CanonicalIdent> for String {
    fn from(canonical: CanonicalIdent) -> Self {
        canonical.0
    }
}

impl From<CanonicalDimensionName> for DimensionName {
    fn from(canonical: CanonicalDimensionName) -> Self {
        canonical.0
    }
}

impl From<CanonicalElementName> for ElementName {
    fn from(canonical: CanonicalElementName) -> Self {
        canonical.0
    }
}

// AsRef implementations for convenient use in APIs
impl AsRef<str> for CanonicalIdent {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RawIdent {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for CanonicalDimensionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RawDimensionName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for CanonicalElementName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RawElementName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

pub fn topo_sort<'out>(
    runlist: Vec<&'out CanonicalIdent>,
    dependencies: &'out HashMap<CanonicalIdent, BTreeSet<CanonicalIdent>>,
) -> Vec<&'out CanonicalIdent> {
    use std::collections::HashSet;

    let runlist_len = runlist.len();
    let mut result: Vec<&'out CanonicalIdent> = Vec::with_capacity(runlist_len);
    let mut used: HashSet<&CanonicalIdent> = HashSet::new();

    // We want to do a postorder, recursive traversal of variables to ensure
    // dependencies are calculated before the variables that reference them.
    // By this point, we have already errored out if we have e.g. a cycle
    fn add<'a>(
        dependencies: &'a HashMap<CanonicalIdent, BTreeSet<CanonicalIdent>>,
        result: &mut Vec<&'a CanonicalIdent>,
        used: &mut HashSet<&'a CanonicalIdent>,
        ident: &'a CanonicalIdent,
    ) {
        if used.contains(ident) {
            return;
        }
        used.insert(ident);
        if let Some(deps) = dependencies.get(ident) {
            for dep in deps.iter() {
                add(dependencies, result, used, dep)
            }
        } else {
            panic!("internal compiler error: unknown ident {}", ident.as_str());
        }
        result.push(ident);
    }

    for ident in runlist.into_iter() {
        add(dependencies, &mut result, &mut used, ident);
    }

    assert_eq!(runlist_len, result.len());
    result
}
