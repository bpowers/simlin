// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim builtin function recognition.
//!
//! This module provides XMUtil-compatible name canonicalization via `to_lower_space`
//! and a lookup table of built-in function names.

use std::collections::HashSet;
use std::sync::LazyLock;

/// Canonicalize a name using XMUtil's ToLowerSpace algorithm.
///
/// This follows the C++ implementation in `SymbolNameSpace.cpp:81-123`:
/// 1. Strip surrounding quotes if present
/// 2. Skip leading whitespace (space, underscore, tab, newline, CR)
/// 3. For each character:
///    - `\_` (escaped underscore) -> keep literally as `\_`
///    - Whitespace (space, `_`, tab, `\n`, `\r`) -> collapse consecutive to single space
///    - Otherwise -> keep character
/// 4. Strip trailing whitespace
/// 5. Lowercase the result
pub fn to_lower_space(s: &str) -> String {
    let bytes = s.as_bytes();
    let len = bytes.len();

    // Step 1: Strip surrounding quotes if present
    let s = if len > 1 && bytes[0] == b'"' && bytes[len - 1] == b'"' {
        &s[1..len - 1]
    } else {
        s
    };

    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    // Step 2: Skip leading whitespace
    while let Some(&c) = chars.peek() {
        if c != ' ' && c != '_' && c != '\t' && c != '\n' && c != '\r' {
            break;
        }
        chars.next();
    }

    // Step 3: Process characters with inline lowercasing
    while let Some(c) = chars.next() {
        // Escaped underscore: \_
        if c == '\\' && chars.peek() == Some(&'_') {
            result.push('\\');
            result.push('_');
            chars.next();
            continue;
        }

        // Whitespace collapse
        if c == '_' || c == ' ' || c == '\t' || c == '\n' || c == '\r' {
            while let Some(&next) = chars.peek() {
                if next != ' ' && next != '_' && next != '\t' && next != '\n' && next != '\r' {
                    break;
                }
                chars.next();
            }
            result.push(' ');
            continue;
        }

        // Lowercase inline
        if c.is_ascii() {
            result.push(c.to_ascii_lowercase());
        } else {
            for lc in c.to_lowercase() {
                result.push(lc);
            }
        }
    }

    // Step 4: Strip trailing whitespace in-place
    let trimmed_len = result.trim_end_matches([' ', '_', '\t', '\n', '\r']).len();
    result.truncate(trimmed_len);

    result
}

/// Built-in function names in their canonicalized form (via `to_lower_space`).
///
/// This table is derived from the C++ `Function.h` class definitions.
static BUILTINS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Mathematical functions
        "abs",
        "exp",
        "sqrt",
        "ln",
        "log",
        "sin",
        "cos",
        "tan",
        "arcsin",
        "arccos",
        "arctan",
        "integer",
        "modulo",
        "quantum",
        // Min/Max
        "max",
        "min",
        "vmax",
        "vmin",
        // Conditional/Division
        "if then else",
        "zidz",
        "xidz",
        // Time functions
        "pulse",
        "pulse train",
        "step",
        "ramp",
        // Delay/Smooth functions
        "smooth",
        "smoothi",
        "smooth3",
        "smooth3i",
        "smooth n",
        "delay1",
        "delay1i",
        "delay3",
        "delay3i",
        "delay fixed",
        "delay n",
        "delay conveyor",
        "trend",
        "forecast",
        // Integration/State
        "integ",
        "active initial",
        "initial",
        "reinitial",
        "sample if true",
        // Lookup functions
        "with lookup",
        "lookup invert",
        "lookup area",
        "lookup extrapolate",
        "lookup forward",
        "lookup backward",
        "tabxl",
        "get data at time",
        "get data last time",
        // Array functions
        "sum",
        "prod",
        "elmcount",
        "vector select",
        "vector elm map",
        "vector sort order",
        "vector reorder",
        "vector lookup",
        // Random functions
        "random 0 1",
        "random uniform",
        "random normal",
        "random pink noise",
        "random poisson",
        // Special
        "a function of",
        "game",
        "time base",
        "npv",
        "allocate by priority",
        "get direct data",
        "get data mean",
        // Keyword (handled specially, but included for completeness)
        "tabbed array",
    ]
    .into_iter()
    .collect()
});

/// Classification of a symbol token after canonicalization.
///
/// Used by the normalizer to classify a symbol in a single `to_lower_space`
/// call rather than checking each category separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolClass {
    /// "WITH LOOKUP" keyword
    WithLookup,
    /// GET XLS/VDF/DATA/DIRECT/123 function, with the canonical prefix
    GetXls(&'static str),
    /// "TABBED ARRAY" keyword
    TabbedArray,
    /// Known builtin function
    Builtin,
    /// Regular symbol (not a builtin or keyword)
    Regular,
}

/// Classify a symbol by canonicalizing once and checking all categories.
pub fn classify_symbol(name: &str) -> SymbolClass {
    let canonical = to_lower_space(name);
    if canonical == "with lookup" {
        return SymbolClass::WithLookup;
    }
    if canonical == "tabbed array" {
        return SymbolClass::TabbedArray;
    }
    if let Some(rest) = canonical.strip_prefix("get ") {
        if rest.starts_with("123") {
            return SymbolClass::GetXls("{GET 123");
        }
        if rest.starts_with("data") {
            return SymbolClass::GetXls("{GET DATA");
        }
        if rest.starts_with("direct") {
            return SymbolClass::GetXls("{GET DIRECT");
        }
        if rest.starts_with("vdf") {
            return SymbolClass::GetXls("{GET VDF");
        }
        if rest.starts_with("xls") {
            return SymbolClass::GetXls("{GET XLS");
        }
    }
    if BUILTINS.contains(canonical.as_str()) {
        return SymbolClass::Builtin;
    }
    SymbolClass::Regular
}

#[cfg(test)]
mod tests {
    use super::*;

    // Phase 1: ToLowerSpace Canonicalization Tests

    #[test]
    fn test_to_lower_space_simple() {
        assert_eq!(to_lower_space("ABC"), "abc");
        assert_eq!(to_lower_space("foo"), "foo");
        assert_eq!(to_lower_space("FooBar"), "foobar");
    }

    #[test]
    fn test_to_lower_space_underscores() {
        assert_eq!(to_lower_space("IF_THEN_ELSE"), "if then else");
        assert_eq!(to_lower_space("my_variable"), "my variable");
        assert_eq!(to_lower_space("a_b_c"), "a b c");
    }

    #[test]
    fn test_to_lower_space_multiple_spaces() {
        assert_eq!(to_lower_space("IF  THEN  ELSE"), "if then else");
        assert_eq!(to_lower_space("a     b"), "a b");
    }

    #[test]
    fn test_to_lower_space_mixed_whitespace() {
        assert_eq!(to_lower_space("IF\t_\nTHEN"), "if then");
        assert_eq!(to_lower_space("a \t_\n\r b"), "a b");
    }

    #[test]
    fn test_to_lower_space_escaped_underscore() {
        // Escaped underscore \_ should be preserved literally
        assert_eq!(to_lower_space("foo\\_bar"), "foo\\_bar");
        assert_eq!(to_lower_space("a\\_b\\_c"), "a\\_b\\_c");
    }

    #[test]
    fn test_to_lower_space_leading_trailing() {
        assert_eq!(to_lower_space("  foo  "), "foo");
        assert_eq!(to_lower_space("__foo__"), "foo");
        assert_eq!(to_lower_space("\t\nfoo\r\n"), "foo");
        assert_eq!(to_lower_space("   "), "");
    }

    #[test]
    fn test_to_lower_space_quoted_strings() {
        // Surrounding quotes should be stripped
        assert_eq!(to_lower_space("\"foo\""), "foo");
        assert_eq!(to_lower_space("\"MY_VAR\""), "my var");
        // Single char in quotes
        assert_eq!(to_lower_space("\"x\""), "x");
    }

    #[test]
    fn test_to_lower_space_empty() {
        assert_eq!(to_lower_space(""), "");
        assert_eq!(to_lower_space("\"\""), "");
    }

    #[test]
    fn test_to_lower_space_non_ascii() {
        assert_eq!(to_lower_space("Foo\u{00B7}Bar"), "foo\u{00b7}bar");
    }

    #[test]
    fn test_classify_symbol() {
        assert!(matches!(
            classify_symbol("WITH LOOKUP"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("WITH_LOOKUP"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("with lookup"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("TABBED ARRAY"),
            SymbolClass::TabbedArray
        ));
        assert!(matches!(
            classify_symbol("tabbed_array"),
            SymbolClass::TabbedArray
        ));
        assert!(matches!(classify_symbol("GET XLS"), SymbolClass::GetXls(_)));
        assert!(matches!(classify_symbol("GET_VDF"), SymbolClass::GetXls(_)));
        assert!(matches!(
            classify_symbol("GET DIRECT DATA"),
            SymbolClass::GetXls(_)
        ));
        assert!(matches!(classify_symbol("MAX"), SymbolClass::Builtin));
        assert!(matches!(classify_symbol("integ"), SymbolClass::Builtin));
        assert!(matches!(
            classify_symbol("IF_THEN_ELSE"),
            SymbolClass::Builtin
        ));
        assert!(matches!(
            classify_symbol("my_variable"),
            SymbolClass::Regular
        ));
        assert!(matches!(classify_symbol("foo"), SymbolClass::Regular));
    }

    // Phase 4: Function Classification Tests

    #[test]
    fn test_max_is_builtin() {
        assert!(matches!(classify_symbol("MAX"), SymbolClass::Builtin));
        assert!(matches!(classify_symbol("max"), SymbolClass::Builtin));
        assert!(matches!(classify_symbol("Max"), SymbolClass::Builtin));
    }

    #[test]
    fn test_function_case_insensitive() {
        assert!(matches!(classify_symbol("INTEG"), SymbolClass::Builtin));
        assert!(matches!(classify_symbol("integ"), SymbolClass::Builtin));
        assert!(matches!(classify_symbol("Integ"), SymbolClass::Builtin));
        assert!(matches!(classify_symbol("SMOOTH3"), SymbolClass::Builtin));
        assert!(matches!(classify_symbol("smooth3"), SymbolClass::Builtin));
    }

    #[test]
    fn test_function_with_underscores() {
        assert!(matches!(
            classify_symbol("IF_THEN_ELSE"),
            SymbolClass::Builtin
        ));
        assert!(matches!(
            classify_symbol("if_then_else"),
            SymbolClass::Builtin
        ));
        assert!(matches!(
            classify_symbol("PULSE_TRAIN"),
            SymbolClass::Builtin
        ));
    }

    #[test]
    fn test_function_with_spaces() {
        assert!(matches!(
            classify_symbol("IF THEN ELSE"),
            SymbolClass::Builtin
        ));
        assert!(matches!(
            classify_symbol("if then else"),
            SymbolClass::Builtin
        ));
        assert!(matches!(
            classify_symbol("DELAY FIXED"),
            SymbolClass::Builtin
        ));
        assert!(matches!(
            classify_symbol("RANDOM UNIFORM"),
            SymbolClass::Builtin
        ));
    }

    #[test]
    fn test_non_function_stays_symbol() {
        assert!(matches!(
            classify_symbol("my_variable"),
            SymbolClass::Regular
        ));
        assert!(matches!(
            classify_symbol("NOT_A_FUNCTION"),
            SymbolClass::Regular
        ));
        assert!(matches!(classify_symbol("foo"), SymbolClass::Regular));
    }

    #[test]
    fn test_tabbed_array_keyword() {
        assert!(matches!(
            classify_symbol("TABBED ARRAY"),
            SymbolClass::TabbedArray
        ));
        assert!(matches!(
            classify_symbol("tabbed_array"),
            SymbolClass::TabbedArray
        ));
        assert!(matches!(
            classify_symbol("Tabbed_Array"),
            SymbolClass::TabbedArray
        ));
        assert!(!matches!(
            classify_symbol("TABBED"),
            SymbolClass::TabbedArray
        ));
        assert!(!matches!(
            classify_symbol("ARRAY"),
            SymbolClass::TabbedArray
        ));
    }

    #[test]
    fn test_with_lookup_keyword() {
        // All spacing variants should match
        assert!(matches!(
            classify_symbol("WITH LOOKUP"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("with lookup"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("WITH_LOOKUP"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("with_lookup"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("WITH  LOOKUP"),
            SymbolClass::WithLookup
        ));
        assert!(matches!(
            classify_symbol("WITH\tLOOKUP"),
            SymbolClass::WithLookup
        ));
        // Non-matches
        assert!(!matches!(classify_symbol("WITH"), SymbolClass::WithLookup));
        assert!(!matches!(
            classify_symbol("LOOKUP"),
            SymbolClass::WithLookup
        ));
        assert!(!matches!(
            classify_symbol("WITHLOOKUP"),
            SymbolClass::WithLookup
        ));
    }
}
