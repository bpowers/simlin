// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim MDL file parser.
//!
//! This module provides a pure Rust implementation for parsing Vensim MDL files
//! directly into `simlin_core::datamodel::Project` structures, replacing the
//! C++ xmutil dependency.
//!
//! See `CLAUDE.md` in this directory for implementation context and goals.

pub mod ast;
mod builtins;
mod convert;
mod lexer;
mod normalizer;
mod parser_helpers;
mod reader;
mod settings;
pub mod view;
mod xmile_compat;

// LALRPOP-generated parser module
use lalrpop_util::lalrpop_mod;
lalrpop_mod!(
    #[allow(clippy::all)]
    #[allow(unused)]
    parser,
    "/mdl/parser.rs"
);

// Public re-exports
pub use lexer::{LexError, LexErrorCode, RawLexer, RawToken, Spanned};
pub use normalizer::{NormalizerError, NormalizerErrorCode, Token, TokenNormalizer};
pub use reader::{EquationReader, ReaderError};

use simlin_core::datamodel::Project;
use simlin_core::{Error, ErrorCode, ErrorKind, Result};

use convert::convert_mdl;

/// Parse a Vensim MDL file into a Project.
///
/// This is the main entry point for MDL parsing. It takes the MDL source as a
/// string and converts it to the internal datamodel representation.
pub fn parse_mdl(source: &str) -> Result<Project> {
    convert_mdl(source).map_err(|e| {
        Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!("Failed to parse MDL: {:?}", e)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mdl_simple() {
        let mdl = "x = 5
~ Units
~ A constant |
\\\\\\---///
";
        let result = parse_mdl(mdl);
        assert!(result.is_ok(), "parse_mdl should succeed: {:?}", result);
        let project = result.unwrap();
        assert_eq!(project.models.len(), 1);
    }

    #[test]
    fn test_parse_mdl_stock() {
        let mdl = "Stock = INTEG(inflow - outflow, 100)
~ Units
~ A stock |
inflow = 10
~ Units/Time
~ Inflow rate |
outflow = 5
~ Units/Time
~ Outflow rate |
\\\\\\---///
";
        let result = parse_mdl(mdl);
        assert!(result.is_ok(), "parse_mdl should succeed: {:?}", result);
        let project = result.unwrap();
        assert_eq!(project.models.len(), 1);
        assert!(!project.models[0].variables.is_empty());

        // Verify stock has inflows/outflows
        use simlin_core::datamodel::Variable;
        let stock = project.models[0]
            .variables
            .iter()
            .find(|v| matches!(v, Variable::Stock(_)));
        assert!(stock.is_some(), "Should have a stock variable");
        if let Some(Variable::Stock(s)) = stock {
            assert_eq!(s.inflows, vec!["inflow"]);
            assert_eq!(s.outflows, vec!["outflow"]);
        }
    }
}
