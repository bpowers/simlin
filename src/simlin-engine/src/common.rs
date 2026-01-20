// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use crate::ast::Loc;

// Re-export all common types from simlin-core
pub use simlin_core::common::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnitError {
    DefinitionError(EquationError, Option<String>),
    ConsistencyError(ErrorCode, Loc, Option<String>),
    /// For inference errors that may span multiple variables.
    /// Each source is (variable_identifier, optional_location_in_that_equation).
    InferenceError {
        code: ErrorCode,
        sources: Vec<(String, Option<Loc>)>,
        details: Option<String>,
    },
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
            UnitError::InferenceError {
                code,
                sources,
                details,
            } => {
                // Format sources as "var@loc" or just "var" if no location
                let sources_str = if sources.is_empty() {
                    "unknown".to_string()
                } else {
                    sources
                        .iter()
                        .map(|(var, loc)| {
                            if let Some(loc) = loc {
                                format!("'{var}'@{loc}")
                            } else {
                                format!("'{var}'")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                if let Some(details) = details {
                    write!(f, "unit inference [{sources_str}]: {code} -- {details}")
                } else {
                    write!(f, "unit inference [{sources_str}]: {code}")
                }
            }
        }
    }
}

pub type UnitResult<T> = std::result::Result<T, UnitError>;

// Macros for error creation - these need to stay in simlin-engine
// as they use crate-local paths

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
        Err(Error::new(
            ErrorKind::Model,
            ErrorCode::$code,
            Some($str),
        ))
    }}
);

#[macro_export]
macro_rules! sim_err {
    ($code:tt, $str:expr) => {{
        use $crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(
            ErrorKind::Simulation,
            ErrorCode::$code,
            Some($str),
        ))
    }};
    ($code:tt) => {{
        use $crate::common::{Error, ErrorCode, ErrorKind};
        Err(Error::new(ErrorKind::Simulation, ErrorCode::$code, None))
    }};
}

#[test]
fn test_unit_error_inference_display() {
    use crate::ast::Loc;

    // Test InferenceError with no sources (edge case)
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![],
        details: None,
    };
    let display = format!("{err}");
    assert!(
        display.contains("unknown"),
        "Empty sources should show 'unknown'"
    );
    assert!(display.contains("unit_mismatch"));

    // Test InferenceError with single source, no location
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![("my_var".to_string(), None)],
        details: None,
    };
    let display = format!("{err}");
    assert!(display.contains("'my_var'"), "Should contain variable name");
    assert!(!display.contains("@"), "Should not have @ when no location");

    // Test InferenceError with single source, with location
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![("my_var".to_string(), Some(Loc::new(5, 10)))],
        details: None,
    };
    let display = format!("{err}");
    assert!(
        display.contains("'my_var'@"),
        "Should contain variable with @ for location"
    );
    assert!(
        display.contains("5:10"),
        "Should contain location 5:10, got: {}",
        display
    );

    // Test InferenceError with multiple sources
    let err = UnitError::InferenceError {
        code: ErrorCode::UnitMismatch,
        sources: vec![
            ("var_a".to_string(), Some(Loc::new(0, 5))),
            ("var_b".to_string(), None),
        ],
        details: Some("conflicting units".to_string()),
    };
    let display = format!("{err}");
    assert!(display.contains("'var_a'@"));
    assert!(display.contains("'var_b'"));
    assert!(
        display.contains(", "),
        "Should have comma-separated sources"
    );
    assert!(
        display.contains("conflicting units"),
        "Should contain details"
    );
    assert!(
        display.contains("--"),
        "Should have -- separator for details"
    );
}

pub fn topo_sort<'out>(
    runlist: Vec<&'out Ident<Canonical>>,
    dependencies: &'out HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
) -> Vec<&'out Ident<Canonical>> {
    use std::collections::HashSet;

    let runlist_len = runlist.len();
    let mut result: Vec<&'out Ident<Canonical>> = Vec::with_capacity(runlist_len);
    let mut used: HashSet<&Ident<Canonical>> = HashSet::new();

    // We want to do a postorder, recursive traversal of variables to ensure
    // dependencies are calculated before the variables that reference them.
    // By this point, we have already errored out if we have e.g. a cycle
    fn add<'a>(
        dependencies: &'a HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
        result: &mut Vec<&'a Ident<Canonical>>,
        used: &mut HashSet<&'a Ident<Canonical>>,
        ident: &'a Ident<Canonical>,
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
