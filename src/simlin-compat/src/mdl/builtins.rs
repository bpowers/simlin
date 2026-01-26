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
    let n = bytes.len();

    // Step 1: Strip surrounding quotes if present
    let s = if n > 1 && bytes[0] == b'"' && bytes[n - 1] == b'"' {
        &s[1..s.len() - 1]
    } else {
        s
    };

    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut result = String::with_capacity(n);

    // Step 2: Skip leading whitespace
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c != ' ' && c != '_' && c != '\t' && c != '\n' && c != '\r' {
            break;
        }
        i += 1;
    }

    // Step 3: Process characters
    while i < n {
        let c = chars[i];

        // Check for escaped underscore: \_
        if c == '\\' && i + 1 < n && chars[i + 1] == '_' {
            result.push('\\');
            result.push('_');
            i += 2;
            continue;
        }

        // Whitespace handling: collapse consecutive whitespace to single space
        if c == '_' || c == ' ' || c == '\t' || c == '\n' || c == '\r' {
            // Skip all consecutive whitespace characters
            while i + 1 < n {
                let next = chars[i + 1];
                if next != ' ' && next != '_' && next != '\t' && next != '\n' && next != '\r' {
                    break;
                }
                i += 1;
            }
            result.push(' ');
            i += 1;
            continue;
        }

        result.push(c);
        i += 1;
    }

    // Step 4: Strip trailing whitespace
    while result.ends_with(' ')
        || result.ends_with('_')
        || result.ends_with('\t')
        || result.ends_with('\n')
        || result.ends_with('\r')
    {
        result.pop();
    }

    // Step 5: Lowercase the result
    result.to_lowercase()
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

/// Check if a name (after canonicalization) is a built-in function.
pub fn is_builtin(name: &str) -> bool {
    let canonical = to_lower_space(name);
    BUILTINS.contains(canonical.as_str())
}

/// Check if a name is the "TABBED ARRAY" keyword function.
///
/// This is special because it requires different handling during parsing.
pub fn is_tabbed_array(name: &str) -> bool {
    to_lower_space(name) == "tabbed array"
}

/// Check if a name is "WITH LOOKUP" (any spacing variant).
///
/// Uses `to_lower_space` canonicalization so "WITH LOOKUP", "WITH_LOOKUP",
/// "WITH  LOOKUP", etc. all match. This is needed because WITH LOOKUP has
/// special syntax (inline table as second argument) that the parser must handle.
pub fn is_with_lookup(name: &str) -> bool {
    to_lower_space(name) == "with lookup"
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

    // Phase 4: Function Classification Tests

    #[test]
    fn test_max_is_builtin() {
        assert!(is_builtin("MAX"));
        assert!(is_builtin("max"));
        assert!(is_builtin("Max"));
    }

    #[test]
    fn test_function_case_insensitive() {
        assert!(is_builtin("INTEG"));
        assert!(is_builtin("integ"));
        assert!(is_builtin("Integ"));
        assert!(is_builtin("SMOOTH3"));
        assert!(is_builtin("smooth3"));
    }

    #[test]
    fn test_function_with_underscores() {
        assert!(is_builtin("IF_THEN_ELSE"));
        assert!(is_builtin("if_then_else"));
        assert!(is_builtin("PULSE_TRAIN"));
    }

    #[test]
    fn test_function_with_spaces() {
        assert!(is_builtin("IF THEN ELSE"));
        assert!(is_builtin("if then else"));
        assert!(is_builtin("DELAY FIXED"));
        assert!(is_builtin("RANDOM UNIFORM"));
    }

    #[test]
    fn test_non_function_stays_symbol() {
        assert!(!is_builtin("my_variable"));
        assert!(!is_builtin("NOT_A_FUNCTION"));
        assert!(!is_builtin("foo"));
    }

    #[test]
    fn test_tabbed_array_keyword() {
        assert!(is_tabbed_array("TABBED ARRAY"));
        assert!(is_tabbed_array("tabbed_array"));
        assert!(is_tabbed_array("Tabbed_Array"));
        assert!(!is_tabbed_array("TABBED"));
        assert!(!is_tabbed_array("ARRAY"));
    }

    #[test]
    fn test_with_lookup_keyword() {
        // All spacing variants should match
        assert!(is_with_lookup("WITH LOOKUP"));
        assert!(is_with_lookup("with lookup"));
        assert!(is_with_lookup("WITH_LOOKUP"));
        assert!(is_with_lookup("with_lookup"));
        assert!(is_with_lookup("WITH  LOOKUP"));
        assert!(is_with_lookup("WITH\tLOOKUP"));
        // Non-matches
        assert!(!is_with_lookup("WITH"));
        assert!(!is_with_lookup("LOOKUP"));
        assert!(!is_with_lookup("WITHLOOKUP"));
    }
}
