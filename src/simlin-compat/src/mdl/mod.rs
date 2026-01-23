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
mod lexer;
mod normalizer;
mod parser_helpers;
mod reader;

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

use std::io::BufRead;

use simlin_core::datamodel::Project;
use simlin_core::{Error, ErrorCode, ErrorKind, Result};

/// Parse a Vensim MDL file into a Project.
///
/// This is the main entry point for MDL parsing. It reads the entire MDL file,
/// parses it, and converts it to the internal datamodel representation.
pub fn parse_mdl(_reader: &mut dyn BufRead) -> Result<Project> {
    // TODO: Implement
    Err(Error::new(
        ErrorKind::Import,
        ErrorCode::Generic,
        Some("MDL parsing not yet implemented".to_owned()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn test_parse_mdl_stub() {
        let mdl = b"";
        let mut reader = BufReader::new(&mdl[..]);
        let result = parse_mdl(&mut reader);
        // For now, just verify the stub returns the expected error
        assert!(result.is_err());
    }
}
