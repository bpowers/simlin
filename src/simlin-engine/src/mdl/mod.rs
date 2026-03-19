// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim MDL file parser.
//!
//! This module provides a pure Rust implementation for parsing Vensim MDL files
//! directly into `crate::datamodel::Project` structures, replacing the
//! C++ xmutil dependency.
//!
//! See `CLAUDE.md` in this directory for implementation context and goals.

pub mod ast;
mod builtins;
mod convert;
mod lexer;
mod normalizer;
mod parser;
mod reader;
mod settings;
pub mod view;
pub mod writer;
mod xmile_compat;

// Public re-exports
pub use lexer::{LexError, LexErrorCode, RawLexer, RawToken, Spanned};
pub use normalizer::{NormalizerError, NormalizerErrorCode, Token, TokenNormalizer};
pub use reader::{EquationReader, ReaderError};
pub use writer::expr0_to_mdl;

use crate::common::{Error, ErrorCode, ErrorKind, Result};
use crate::datamodel::{Project, Variable};

use convert::convert_mdl_with_data;
use writer::MdlWriter;

/// Sentinel equation produced by the MDL parser for variables that are
/// pure lookup definitions (no input expression) or have an empty RHS.
/// The writer recognises this to emit native Vensim `name(body)` syntax
/// instead of `name = WITH LOOKUP(input, body)`.
pub(crate) const LOOKUP_SENTINEL: &str = "0+0";

/// Convert a Project to Vensim MDL text.
pub fn project_to_mdl(project: &Project) -> Result<String> {
    if project.models.len() != 1 {
        return Err(Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some("MDL format supports only a single model".to_owned()),
        ));
    }

    let model = &project.models[0];
    for var in &model.variables {
        if matches!(var, Variable::Module(_)) {
            return Err(Error::new(
                ErrorKind::Import,
                ErrorCode::Generic,
                Some("MDL format does not support Module variables".to_owned()),
            ));
        }
    }

    let writer = MdlWriter::new();
    writer.write_project(project)
}

/// Parse a Vensim MDL file into a Project.
///
/// This is the main entry point for MDL parsing. It takes the MDL source as a
/// string and converts it to the internal datamodel representation.
pub fn parse_mdl(source: &str) -> Result<Project> {
    parse_mdl_with_data(source, None)
}

/// Parse a Vensim MDL file into a Project with an optional DataProvider
/// for resolving GET DIRECT external data references.
pub fn parse_mdl_with_data(
    source: &str,
    data_provider: Option<&dyn crate::data_provider::DataProvider>,
) -> Result<Project> {
    convert_mdl_with_data(source, data_provider).map_err(|e| {
        Error::new(
            ErrorKind::Import,
            ErrorCode::Generic,
            Some(format!("Failed to parse MDL: {}", e)),
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
        use crate::datamodel::Variable;
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
