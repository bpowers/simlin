// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What every simlin crate is built on: the data model and the patches to it,
//! identifiers, errors, dimensions, the equation lexer, parser and AST, the
//! builtin functions and the standard library of module functions, and the
//! results of a run.

// deny (not forbid): `rapidhash` views a slice of integers as bytes, its one
// opt-in.
#![deny(unsafe_code)]

pub mod alloc;
pub mod ast;
pub mod builtins;
pub mod common;
pub mod datamodel;
pub mod dimensions;
pub mod float;
pub mod lexer;
pub mod module_functions;
pub mod parser;
pub mod project_patch;
#[doc(hidden)]
pub mod rapidhash;
pub mod results;
pub mod shared_vec;
#[path = "stdlib.gen.rs"]
pub mod stdlib;

pub use self::common::{Error, ErrorCode, ErrorKind, Result, canonicalize};
pub use self::project_patch::{ModelOperation, ModelPatch, ProjectOperation, ProjectPatch};
pub use self::results::{Method, Results, Specs as SimSpecs};

#[cfg(test)]
mod stdlib_freshness_tests {
    use sha2::{Digest, Sha256};
    use std::fs;

    const GEN_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/stdlib.gen.rs");
    const STDLIB_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../stdlib");

    fn extract_hash_from_gen_file(content: &str) -> Option<&str> {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("// Stdlib SHA256: ") {
                return Some(rest.trim());
            }
        }
        None
    }

    #[test]
    fn stdlib_gen_is_up_to_date() {
        let gen_content = fs::read_to_string(GEN_FILE)
            .expect("failed to read stdlib.gen.rs - run `pnpm rebuild-stdlib`");

        let recorded_hash = extract_hash_from_gen_file(&gen_content)
            .expect("stdlib.gen.rs is missing SHA256 hash header");

        let mut hasher = Sha256::new();
        let mut entries: Vec<_> = fs::read_dir(STDLIB_DIR)
            .expect("failed to read stdlib directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("failed to read directory entry")
            .into_iter()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "stmx"))
            .collect();
        entries.sort_by_key(|e| e.path());

        for entry in entries {
            let path = entry.path();
            let file_stem = path
                .file_stem()
                .expect("stmx file should have a file stem")
                .to_string_lossy();
            hasher.update(file_stem.as_bytes());
            hasher.update(fs::read(&path).expect("failed to read stmx file"));
        }
        let current_hash = format!("{:x}", hasher.finalize());

        assert_eq!(
            recorded_hash, current_hash,
            "stdlib/*.stmx files have changed since stdlib.gen.rs was generated.\n\
             Run `pnpm rebuild-stdlib` to regenerate."
        );
    }
}
