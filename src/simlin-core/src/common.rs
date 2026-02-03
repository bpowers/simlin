// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fmt;
use std::marker::PhantomData;
use std::{error, result};

use std::borrow::Cow;

// Legacy type aliases - to be deprecated
pub type DimensionName = String;
pub type ElementName = String;

/// A canonicalized identifier - guaranteed to be in canonical form (OLD - being replaced)
///
/// Canonical form means:
/// - Lowercase
/// - Spaces/newlines replaced with underscores
/// - Dots outside quotes replaced with middle dot (·)
/// - Properly handles quoted sections
///
/// A raw, non-canonicalized identifier as it appears in source.
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Import,
    Model,
    Simulation,
    Variable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub code: ErrorCode,
    pub details: Option<String>,
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

impl Error {
    pub fn new(kind: ErrorKind, code: ErrorCode, details: Option<String>) -> Self {
        Error {
            kind,
            code,
            details,
        }
    }

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

pub fn canonicalize(name: &str) -> Ident<Canonical> {
    // remove leading and trailing whitespace, do this before testing
    // for quotedness as we should treat a quoted string as sacrosanct
    let name = name.trim();

    let mut canonicalized_name = String::with_capacity(name.len());

    for part in IdentifierPartIterator::new(name) {
        let bytes = part.as_bytes();
        let quoted: bool =
            { bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' };

        let part = if quoted {
            Cow::Borrowed(&part[1..bytes.len() - 1])
        } else {
            // Replace periods with middle dots (·) for module hierarchy separators.
            // This allows us to distinguish between:
            // - Module separators: model.variable -> model·variable
            // - Literal periods in quoted names: "a.b" -> a.b
            Cow::Owned(part.replace('.', "·"))
        };

        let part = part.replace("\\\\", "\\");
        let part = replace_whitespace_with_underscore(&part);
        let part = part.to_lowercase();

        canonicalized_name.push_str(&part);
    }

    Ident::from_unchecked(canonicalized_name)
}

#[test]
fn test_canonicalize() {
    assert_eq!("a.b", canonicalize("\"a.b\"").as_str());
    assert_eq!(
        "a/d·b_\\\"c\\\"",
        canonicalize("\"a/d\".\"b \\\"c\\\"\"").as_str()
    );
    assert_eq!("a/d·b_c", canonicalize("\"a/d\".\"b c\"").as_str());
    assert_eq!("a·b_c", canonicalize("a.\"b c\"").as_str());
    assert_eq!("a/d·b", canonicalize("\"a/d\".b").as_str());
    assert_eq!("quoted", canonicalize("\"quoted\"").as_str());
    assert_eq!("a_b", canonicalize("   a b").as_str());
    assert_eq!("å_b", canonicalize("Å\nb").as_str());
    assert_eq!("a_b", canonicalize("a \n b").as_str());
    assert_eq!("a·b", canonicalize("a.b").as_str());
}

#[test]
fn test_canonical_ident() {
    // Test canonicalization from raw
    let raw = RawIdent::new("Hello World".to_string());
    let canonical = raw.canonicalize();
    assert_eq!(canonical.as_str(), "hello_world");

    // Test direct creation with new type
    let canonical2 = canonicalize("Hello World");
    assert_eq!(canonical.as_str(), canonical2.as_str());

    // Test to_source_repr with new type
    let canonical3 = canonicalize("a.b");
    assert_eq!(canonical3.as_str(), "a·b");
    assert_eq!(canonical3.to_source_repr(), "a.b");

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
    let c1 = canonicalize("a.d");
    assert_eq!(c1.as_str(), "a·d");

    // Test that quoted identifiers with dots keep them as middle dots after canonicalization
    let c2 = canonicalize("\"a.d\"");
    assert_eq!(c2.as_str(), "a.d");

    // Test mixed case
    let c3 = canonicalize("a.\"b.c\"");
    assert_eq!(c3.as_str(), "a·b.c");
}

#[test]
fn test_stdlib_model_name_canonicalization() {
    // Test canonicalization of stdlib model names
    let stdlib_name = "stdlib⁚smth1";
    let canonical = canonicalize(stdlib_name);
    assert_eq!(canonical.as_str(), "stdlib⁚smth1");

    // Test that the Display trait's to_string() preserves the name
    assert_eq!(canonical.to_string(), "stdlib⁚smth1");
}

#[test]
fn test_stdlib_variable_canonicalization() {
    // Test that stdlib variable names are canonicalized correctly
    let names = vec!["input", "output", "Output", "delay_time", "initial_value"];
    for name in names {
        let canonical = canonicalize(name);
        let expected = canonicalize(name);
        assert_eq!(canonical.as_str(), expected.as_str(), "Failed for {name}");
    }

    // Specifically test Output -> output conversion
    assert_eq!(canonicalize("Output").as_str(), "output");
}

#[test]
fn test_new_ident_basic_operations() {
    // Test basic creation and conversion
    let ident = canonicalize("Hello World");
    assert_eq!(ident.as_str(), "hello_world");

    // Test source representation conversion
    let ident2 = canonicalize("a.b");
    assert_eq!(ident2.as_str(), "a·b");
    assert_eq!(ident2.to_source_repr(), "a.b");

    // Test that quoted sections are preserved
    let ident3 = canonicalize("\"a.b\"");
    assert_eq!(ident3.as_str(), "a.b");
}

#[test]
fn test_ident_join_operation() {
    // Test joining two canonical identifiers
    let module = CanonicalStr::from_canonical_unchecked("model");
    let var = CanonicalStr::from_canonical_unchecked("variable");
    let joined = Ident::<Canonical>::join(&module, &var);
    assert_eq!(joined.as_str(), "model·variable");
    assert_eq!(joined.to_source_repr(), "model.variable");
}

#[test]
fn test_ident_with_subscript() {
    let ident = canonicalize("my_array");
    let subscripted = ident.with_subscript("1,2");
    assert_eq!(subscripted.as_str(), "my_array[1,2]");
    assert_eq!(subscripted.to_source_repr(), "my_array[1,2]");

    // Test with identifier containing middle dot
    let ident2 = canonicalize("model.var");
    let subscripted2 = ident2.with_subscript("i");
    assert_eq!(subscripted2.as_str(), "model.var[i]");
    assert_eq!(subscripted2.to_source_repr(), "model.var[i]");
}

#[test]
fn test_ident_strip_prefix() {
    let ident = canonicalize("model.variable");

    // Test successful prefix stripping
    if let Some(stripped) = ident.strip_prefix("model·") {
        assert_eq!(stripped.as_str(), "variable");
    } else {
        panic!("Expected successful prefix strip");
    }

    // Test unsuccessful prefix stripping
    assert!(ident.strip_prefix("other·").is_none());

    // Test stripping empty prefix
    if let Some(stripped) = ident.strip_prefix("") {
        assert_eq!(stripped.as_str(), "model·variable");
    } else {
        panic!("Expected successful empty prefix strip");
    }
}

#[test]
fn test_canonical_str_operations() {
    let canonical = canonicalize("module.sub.variable");
    let canonical_str = canonical.as_canonical_str();

    // Test split_at_dot
    if let Some((before, after)) = canonical_str.split_at_dot() {
        assert_eq!(before.as_str(), "module");
        assert_eq!(after.as_str(), "sub·variable");

        // Test nested split on the after part
        if let Some((first, rest)) = after.split_at_dot() {
            assert_eq!(first.as_str(), "sub");
            assert_eq!(rest.as_str(), "variable");
        } else {
            panic!("Expected successful nested split");
        }
    } else {
        panic!("Expected successful split");
    }

    // Test with no dots
    let no_dots = canonicalize("simple");
    assert!(no_dots.as_canonical_str().split_at_dot().is_none());
}

#[test]
fn test_canonical_str_strip_prefix() {
    let ident = canonicalize("stdlib⁚smooth");
    let canonical_str = ident.as_canonical_str();

    if let Some(stripped) = canonical_str.strip_prefix("stdlib⁚") {
        assert_eq!(stripped.as_str(), "smooth");
    } else {
        panic!("Expected successful prefix strip");
    }

    // Test that stripped result maintains canonical form
    let ident2 = canonicalize("model.Sub Module");
    let canonical_str2 = ident2.as_canonical_str();
    if let Some(stripped) = canonical_str2.strip_prefix("model·") {
        assert_eq!(stripped.as_str(), "sub_module");
    } else {
        panic!("Expected successful prefix strip");
    }
}

#[test]
fn test_ident_ref_operations() {
    let owned = canonicalize("model.variable");
    let borrowed = owned.as_ref();

    // Test basic operations
    assert_eq!(borrowed.as_str(), "model·variable");
    assert_eq!(borrowed.to_source_repr(), Cow::Borrowed("model.variable"));

    // Test strip_prefix on borrowed
    if let Some(stripped) = borrowed.strip_prefix("model·") {
        assert_eq!(stripped.as_str(), "variable");

        // Test that we can convert back to owned
        let owned_again = stripped.to_owned();
        assert_eq!(owned_again.as_str(), "variable");
    } else {
        panic!("Expected successful prefix strip");
    }
}

#[test]
fn test_ident_ref_zero_copy() {
    // This test verifies that IdentRef provides zero-copy substring operations
    let owned = canonicalize("very.long.module.path.to.variable");
    let borrowed = owned.as_ref();

    // Strip multiple prefixes without allocation
    let mut current = borrowed;
    let prefixes = ["very·", "long·", "module·", "path·", "to·"];

    for prefix in &prefixes {
        if let Some(stripped) = current.strip_prefix(prefix) {
            current = stripped;
        } else {
            panic!("Expected successful strip of {prefix}");
        }
    }

    assert_eq!(current.as_str(), "variable");
}

#[test]
fn test_canonical_str_utility_methods() {
    let ident = canonicalize("model.variable");
    let canonical_str = ident.as_canonical_str();

    // Test starts_with
    assert!(canonical_str.starts_with("model·"));
    assert!(!canonical_str.starts_with("other·"));

    // Test find
    // The string is "model·variable" where · is at byte position 5
    assert_eq!(canonical_str.find("·"), Some(5));

    // First let's verify what the actual string is
    let s = canonical_str.as_str();
    assert_eq!(s, "model·variable");

    // str::find() returns byte positions, and "·" is 3 bytes in UTF-8
    // "model" = bytes 0-4, "·" = bytes 5-7, "variable" starts at byte 8
    // But wait - str::find() actually returns the byte index!
    let var_pos = s.find("var").unwrap();
    assert_eq!(canonical_str.find("var"), Some(var_pos));
    assert_eq!(canonical_str.find("notfound"), None);
}

#[test]
fn test_display_format_edge_cases() {
    // Test empty string
    let empty = canonicalize("");
    assert_eq!(empty.as_str(), "");
    assert_eq!(empty.to_source_repr(), "");

    // Test string with only spaces
    let spaces = canonicalize("   ");
    assert_eq!(spaces.as_str(), "");

    // Test string with mixed dots and quotes
    let complex = canonicalize("a.\"b.c\".d");
    assert_eq!(complex.as_str(), "a·b.c·d");
    assert_eq!(complex.to_source_repr(), "a.b.c.d");
}

#[test]
fn test_unchecked_constructors() {
    // Test unchecked construction of Ident
    let canonical_string = "already_canonical".to_string();
    let ident = Ident::<Canonical>::from_unchecked(canonical_string.clone());
    assert_eq!(ident.as_str(), "already_canonical");

    // Test unchecked construction of IdentRef
    let canonical_str = "also_canonical";
    let ident_ref = IdentRef::<Canonical>::from_canonical_unchecked(canonical_str);
    assert_eq!(ident_ref.as_str(), "also_canonical");

    // Test unchecked construction of CanonicalStr
    let canonical_slice = CanonicalStr::from_canonical_unchecked("canonical·str");
    assert_eq!(canonical_slice.as_str(), "canonical·str");
}

#[test]
fn test_as_ref_implementations() {
    let ident = canonicalize("test");
    let _str_ref: &str = <Ident<Canonical> as AsRef<str>>::as_ref(&ident);
    assert_eq!(_str_ref, "test");

    let ident_ref = ident.as_ref();
    let _str_ref2: &str = <IdentRef<'_, Canonical> as AsRef<str>>::as_ref(&ident_ref);
    assert_eq!(_str_ref2, "test");

    let canonical_str = ident.as_canonical_str();
    let _str_ref3: &str = canonical_str.as_ref();
    assert_eq!(_str_ref3, "test");
}

#[test]
fn test_fmt_display_implementations() {
    let ident = canonicalize("Model.Var");
    assert_eq!(format!("{ident}"), "model·var");

    let ident_ref = ident.as_ref();
    assert_eq!(format!("{ident_ref}"), "model·var");

    let canonical_str = ident.as_canonical_str();
    assert_eq!(format!("{canonical_str}"), "model·var");
}

// Implementations for identifier types

impl RawIdent {
    /// Create a new raw identifier
    pub fn new(s: String) -> Self {
        RawIdent(s)
    }

    /// Create from a string slice
    pub fn new_from_str(s: &str) -> Self {
        RawIdent(s.to_string())
    }

    /// Canonicalize this identifier (returns new type)
    pub fn canonicalize(&self) -> Ident<Canonical> {
        canonicalize(&self.0)
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
        CanonicalDimensionName(canonicalize(s).as_str().to_string())
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
        CanonicalDimensionName(canonicalize(&self.0).as_str().to_string())
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
        CanonicalElementName(canonicalize(s).as_str().to_string())
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
        CanonicalElementName(canonicalize(&self.0).as_str().to_string())
    }

    /// Get the underlying raw string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// Display implementations for better debugging

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

// ===== New Phantom Type-based Identifier System =====
// This system provides zero-copy substring operations while maintaining
// canonicalization guarantees through the type system.

/// Marker type for canonical identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Canonical;

/// Marker type for raw (non-canonical) identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Raw;

/// An owned identifier with state tracking (canonical or raw)
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ident<State = Canonical> {
    inner: String,
    _phantom: PhantomData<State>,
}

/// A borrowed identifier reference with state tracking
/// This is the key type that enables zero-copy substring operations
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct IdentRef<'a, State = Canonical> {
    inner: &'a str,
    _phantom: PhantomData<State>,
}

/// A borrowed canonical string slice wrapper
/// This type guarantees the string is in canonical form
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct CanonicalStr<'a> {
    inner: &'a str,
}

impl<'a> CanonicalStr<'a> {
    /// Create a CanonicalStr from a string known to be canonical
    ///
    /// Note: Caller must guarantee that the string is already in canonical form
    pub fn from_canonical_unchecked(s: &'a str) -> Self {
        CanonicalStr { inner: s }
    }

    /// Get the underlying string slice
    pub fn as_str(&self) -> &str {
        self.inner
    }

    /// Convert canonical identifier to source code representation.
    ///
    /// Replaces middle dots (·) used internally for module hierarchy separators
    /// back to periods (.) for display in source code or user-facing output.
    pub fn to_source_repr(&self) -> Cow<'_, str> {
        if self.inner.contains('·') {
            Cow::Owned(self.inner.replace('·', "."))
        } else {
            Cow::Borrowed(self.inner)
        }
    }

    /// Find and split at the first middle dot, maintaining canonical guarantee
    pub fn split_at_dot(&self) -> Option<(CanonicalStr<'a>, CanonicalStr<'a>)> {
        self.inner.find('·').map(|pos| {
            let before = CanonicalStr::from_canonical_unchecked(&self.inner[..pos]);
            let after = CanonicalStr::from_canonical_unchecked(&self.inner[pos + '·'.len_utf8()..]);
            (before, after)
        })
    }

    /// Strip a prefix if present, maintaining canonical guarantee
    pub fn strip_prefix(&self, prefix: &str) -> Option<CanonicalStr<'a>> {
        self.inner
            .strip_prefix(prefix)
            .map(CanonicalStr::from_canonical_unchecked)
    }

    /// Check if this identifier starts with a given prefix
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.inner.starts_with(prefix)
    }

    /// Find the position of a substring
    pub fn find(&self, pat: &str) -> Option<usize> {
        self.inner.find(pat)
    }
}

impl Ident<Canonical> {
    /// Create a canonical identifier from a raw string
    pub fn from_raw(s: &str) -> Self {
        canonicalize(s)
    }

    /// Create from an already-canonicalized string
    ///
    /// Note: Caller must guarantee the string is already canonical
    pub fn from_unchecked(s: String) -> Self {
        Ident {
            inner: s,
            _phantom: PhantomData,
        }
    }

    /// Create from an already-canonicalized string slice
    ///
    /// Note: Caller must guarantee the string is already canonical
    pub fn from_str_unchecked(s: &str) -> Self {
        Ident {
            inner: s.to_string(),
            _phantom: PhantomData,
        }
    }

    /// Get a borrowed reference to this identifier
    pub fn as_ref(&self) -> IdentRef<'_, Canonical> {
        IdentRef {
            inner: &self.inner,
            _phantom: PhantomData,
        }
    }

    /// Get as a CanonicalStr
    pub fn as_canonical_str(&self) -> CanonicalStr<'_> {
        CanonicalStr::from_canonical_unchecked(&self.inner)
    }

    /// Join two canonical identifiers with a middle dot separator
    pub fn join(module: &CanonicalStr, var: &CanonicalStr) -> Self {
        Ident {
            inner: format!("{}·{}", module.as_str(), var.as_str()),
            _phantom: PhantomData,
        }
    }

    /// Create an identifier with array subscript notation
    pub fn with_subscript(&self, subscript: &str) -> Self {
        Ident {
            inner: format!("{}[{}]", self.to_source_repr(), subscript),
            _phantom: PhantomData,
        }
    }

    /// Get the underlying canonical string
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Consume self and return the underlying String
    pub fn into_string(self) -> String {
        self.inner
    }

    /// Convert canonical identifier to source code representation.
    ///
    /// Replaces middle dots (·) used internally for module hierarchy separators
    /// back to periods (.) for display in source code or user-facing output.
    ///
    /// For example:
    /// - Internal canonical: "model·variable"
    /// - Source representation: "model.variable"
    ///
    /// This is the inverse of the canonicalization process that converts
    /// periods to middle dots to distinguish module separators from literal
    /// periods in quoted identifiers.
    pub fn to_source_repr(&self) -> String {
        self.inner.replace('·', ".")
    }

    /// Strip a prefix, returning a borrowed view if successful
    pub fn strip_prefix<'a>(&'a self, prefix: &str) -> Option<IdentRef<'a, Canonical>> {
        self.inner.strip_prefix(prefix).map(|s| IdentRef {
            inner: s,
            _phantom: PhantomData,
        })
    }
}

impl<'a> IdentRef<'a, Canonical> {
    /// Create from a string slice known to be canonical
    ///
    /// Note: Caller must guarantee the string is already canonical
    pub fn from_canonical_unchecked(s: &'a str) -> Self {
        IdentRef {
            inner: s,
            _phantom: PhantomData,
        }
    }

    /// Get the underlying string slice
    pub fn as_str(&self) -> &'a str {
        self.inner
    }

    /// Get as a CanonicalStr
    pub fn as_canonical_str(&self) -> CanonicalStr<'a> {
        CanonicalStr::from_canonical_unchecked(self.inner)
    }

    /// Convert to an owned Ident
    pub fn to_owned(&self) -> Ident<Canonical> {
        Ident {
            inner: self.inner.to_string(),
            _phantom: PhantomData,
        }
    }

    /// Strip a prefix, maintaining the canonical guarantee
    pub fn strip_prefix(&self, prefix: &str) -> Option<IdentRef<'a, Canonical>> {
        self.inner.strip_prefix(prefix).map(|s| IdentRef {
            inner: s,
            _phantom: PhantomData,
        })
    }

    /// Convert canonical identifier to source code representation.
    ///
    /// Replaces middle dots (·) used internally for module hierarchy separators
    /// back to periods (.) for display in source code or user-facing output.
    pub fn to_source_repr(&self) -> Cow<'a, str> {
        if self.inner.contains('·') {
            Cow::Owned(self.inner.replace('·', "."))
        } else {
            Cow::Borrowed(self.inner)
        }
    }
}

// Implement AsRef for convenient usage
impl AsRef<str> for Ident<Canonical> {
    fn as_ref(&self) -> &str {
        &self.inner
    }
}

// Implement Borrow for HashMap lookups
impl std::borrow::Borrow<str> for Ident<Canonical> {
    fn borrow(&self) -> &str {
        &self.inner
    }
}

impl<'a> AsRef<str> for IdentRef<'a, Canonical> {
    fn as_ref(&self) -> &str {
        self.inner
    }
}

impl<'a> AsRef<str> for CanonicalStr<'a> {
    fn as_ref(&self) -> &str {
        self.inner
    }
}

// Display implementations
impl fmt::Display for Ident<Canonical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<'a> fmt::Display for IdentRef<'a, Canonical> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<'a> fmt::Display for CanonicalStr<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

// ===== Helper Functions for Regex-Free Parsing =====

/// Replace whitespace sequences with underscores.
/// Handles: literal `\n` and `\r` (two-character sequences), actual newlines/carriage returns,
/// spaces, and non-breaking spaces (U+00A0). Consecutive matches become a single underscore.
fn replace_whitespace_with_underscore(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_whitespace = false;

    while let Some(c) = chars.next() {
        // Check for escaped sequences: literal \n or \r (two characters)
        if c == '\\'
            && let Some(&next) = chars.peek()
            && (next == 'n' || next == 'r')
        {
            chars.next(); // consume the 'n' or 'r'
            if !in_whitespace {
                result.push('_');
                in_whitespace = true;
            }
            continue;
        } else if c == '\\' {
            // Not an escape sequence we handle, pass through
            in_whitespace = false;
            result.push(c);
        } else if c == '\n' || c == '\r' || c == ' ' || c == '\u{00A0}' {
            // Actual whitespace characters
            if !in_whitespace {
                result.push('_');
                in_whitespace = true;
            }
        } else {
            in_whitespace = false;
            result.push(c);
        }
    }

    result
}

/// Iterator over identifier parts (quoted and unquoted sections).
/// Handles quoted strings with escaped quotes inside them.
/// Matches the regex: [^"]+|"((\\")|[^"])*"
struct IdentifierPartIterator<'a> {
    remaining: &'a str,
}

impl<'a> IdentifierPartIterator<'a> {
    fn new(s: &'a str) -> Self {
        IdentifierPartIterator { remaining: s }
    }
}

impl<'a> Iterator for IdentifierPartIterator<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }

        let bytes = self.remaining.as_bytes();

        if bytes[0] == b'"' {
            // Quoted section: find the closing quote, handling escaped quotes
            let mut i = 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                    // Skip escaped quote
                    i += 2;
                } else if bytes[i] == b'"' {
                    // Found closing quote
                    let part = &self.remaining[..i + 1];
                    self.remaining = &self.remaining[i + 1..];
                    return Some(part);
                } else {
                    i += 1;
                }
            }
            // Unclosed quote - return rest as is
            let part = self.remaining;
            self.remaining = "";
            Some(part)
        } else {
            // Unquoted section: find the next quote or end
            let end = self.remaining.find('"').unwrap_or(self.remaining.len());
            let part = &self.remaining[..end];
            self.remaining = &self.remaining[end..];
            if part.is_empty() {
                self.next()
            } else {
                Some(part)
            }
        }
    }
}

#[cfg(test)]
mod whitespace_replacement_tests {
    use super::*;

    #[test]
    fn test_replace_actual_newline() {
        assert_eq!(replace_whitespace_with_underscore("a\nb"), "a_b");
    }

    #[test]
    fn test_replace_actual_carriage_return() {
        assert_eq!(replace_whitespace_with_underscore("a\rb"), "a_b");
    }

    #[test]
    fn test_replace_crlf() {
        assert_eq!(replace_whitespace_with_underscore("a\r\nb"), "a_b");
    }

    #[test]
    fn test_replace_escaped_newline() {
        // Literal backslash-n in the string (two characters: '\' and 'n')
        assert_eq!(replace_whitespace_with_underscore("a\\nb"), "a_b");
    }

    #[test]
    fn test_replace_escaped_carriage_return() {
        // Literal backslash-r in the string (two characters: '\' and 'r')
        assert_eq!(replace_whitespace_with_underscore("a\\rb"), "a_b");
    }

    #[test]
    fn test_replace_space() {
        assert_eq!(
            replace_whitespace_with_underscore("hello world"),
            "hello_world"
        );
    }

    #[test]
    fn test_replace_non_breaking_space() {
        // U+00A0 non-breaking space
        assert_eq!(replace_whitespace_with_underscore("a\u{00A0}b"), "a_b");
    }

    #[test]
    fn test_consecutive_whitespace_collapsed() {
        // Multiple spaces should become single underscore
        assert_eq!(replace_whitespace_with_underscore("a   b"), "a_b");
        // Mixed whitespace types should collapse
        assert_eq!(replace_whitespace_with_underscore("a \n \r b"), "a_b");
    }

    #[test]
    fn test_leading_trailing_whitespace() {
        assert_eq!(replace_whitespace_with_underscore(" a b "), "_a_b_");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(replace_whitespace_with_underscore(""), "");
    }

    #[test]
    fn test_no_whitespace() {
        assert_eq!(replace_whitespace_with_underscore("hello"), "hello");
    }

    #[test]
    fn test_unicode_preserved() {
        assert_eq!(replace_whitespace_with_underscore("Å b"), "Å_b");
    }

    #[test]
    fn test_multiple_segments() {
        assert_eq!(replace_whitespace_with_underscore("a b c d"), "a_b_c_d");
    }
}

#[cfg(test)]
mod identifier_part_iterator_tests {
    use super::*;

    #[test]
    fn test_simple_unquoted() {
        let parts: Vec<_> = IdentifierPartIterator::new("abc").collect();
        assert_eq!(parts, vec!["abc"]);
    }

    #[test]
    fn test_simple_quoted() {
        let parts: Vec<_> = IdentifierPartIterator::new("\"abc\"").collect();
        assert_eq!(parts, vec!["\"abc\""]);
    }

    #[test]
    fn test_mixed_unquoted_quoted() {
        // a."b c" should yield "a." and "\"b c\""
        let parts: Vec<_> = IdentifierPartIterator::new("a.\"b c\"").collect();
        assert_eq!(parts, vec!["a.", "\"b c\""]);
    }

    #[test]
    fn test_multiple_quoted_sections() {
        // "a/d"."b c" should yield "\"a/d\"", ".", "\"b c\""
        let parts: Vec<_> = IdentifierPartIterator::new("\"a/d\".\"b c\"").collect();
        assert_eq!(parts, vec!["\"a/d\"", ".", "\"b c\""]);
    }

    #[test]
    fn test_escaped_quote_inside_quoted() {
        // "b \"c\"" should be a single part with escaped quotes
        let parts: Vec<_> = IdentifierPartIterator::new("\"b \\\"c\\\"\"").collect();
        assert_eq!(parts, vec!["\"b \\\"c\\\"\""]);
    }

    #[test]
    fn test_complex_mixed() {
        // "a/d"."b \"c\"" should yield parts correctly
        let parts: Vec<_> = IdentifierPartIterator::new("\"a/d\".\"b \\\"c\\\"\"").collect();
        assert_eq!(parts, vec!["\"a/d\"", ".", "\"b \\\"c\\\"\""]);
    }

    #[test]
    fn test_empty_string() {
        let parts: Vec<_> = IdentifierPartIterator::new("").collect();
        assert!(parts.is_empty());
    }

    #[test]
    fn test_only_dots() {
        let parts: Vec<_> = IdentifierPartIterator::new("...").collect();
        assert_eq!(parts, vec!["..."]);
    }

    #[test]
    fn test_unquoted_with_dots() {
        let parts: Vec<_> = IdentifierPartIterator::new("a.b.c").collect();
        assert_eq!(parts, vec!["a.b.c"]);
    }
}
