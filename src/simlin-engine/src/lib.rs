// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

pub use prost;

#[cfg(feature = "ai_info")]
pub mod ai_info;
#[cfg(test)]
mod array_tests;
mod ast;
pub mod builtins;
mod builtins_visitor;
mod bytecode;
pub mod common;
pub mod compat;
mod compiler;
pub mod datamodel;
pub mod diagram;
mod dimensions;
pub mod interpreter;
pub mod json;
#[cfg(test)]
mod json_proptest;
pub mod json_sdai;
#[cfg(test)]
mod json_sdai_proptest;
mod lexer;
pub mod ltm;
pub mod ltm_augment;
pub mod mdl;
mod model;
mod parser;
mod patch;
mod project;
#[allow(clippy::derive_partial_eq_without_eq)]
#[path = "project_io.gen.rs"]
pub mod project_io;
mod results;
pub mod serde;
#[path = "stdlib.gen.rs"]
mod stdlib;
pub mod test_common;
#[cfg(all(test, feature = "xmutil"))]
mod test_open_vensim;
#[cfg(test)]
mod test_sir_xmile;
#[cfg(test)]
mod testutils;
#[cfg(test)]
mod unit_checking_test;
mod units;
mod units_check;
mod units_infer;
mod variable;
mod vm;
pub mod xmile;

pub use self::common::{Error, ErrorCode, ErrorKind, Result, canonicalize};
pub use self::interpreter::Simulation;
pub use self::model::{ModelStage1, resolve_non_private_dependencies};
pub use self::patch::{ModelOperation, ModelPatch, ProjectOperation, ProjectPatch, apply_patch};
pub use self::project::Project;
pub use self::results::{Method, Results, Specs as SimSpecs};
pub use self::variable::{Variable, identifier_set};
pub use self::vm::{CompiledSimulation, Vm};

// Re-export compat functions at the crate root for convenience
#[cfg(feature = "xmutil")]
pub use self::compat::open_vensim_xmutil;
#[cfg(feature = "file_io")]
pub use self::compat::{load_csv, load_dat};
pub use self::compat::{open_vensim, open_xmile, to_xmile};

#[cfg(test)]
mod protobuf_freshness_tests {
    use sha2::{Digest, Sha256};
    use std::fs;

    const GEN_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/project_io.gen.rs");
    const PROTO_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/project_io.proto");

    fn extract_hash_from_gen_file(content: &str) -> Option<&str> {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("// Proto file SHA256: ") {
                return Some(rest.trim());
            }
        }
        None
    }

    #[test]
    fn project_io_gen_is_up_to_date() {
        let gen_content = fs::read_to_string(GEN_FILE)
            .expect("failed to read project_io.gen.rs - run `pnpm build:gen-protobufs`");

        let recorded_hash = extract_hash_from_gen_file(&gen_content)
            .expect("project_io.gen.rs is missing SHA256 hash header");

        let proto_content = fs::read(PROTO_FILE).expect("failed to read project_io.proto");
        let mut hasher = Sha256::new();
        hasher.update(&proto_content);
        let current_hash = format!("{:x}", hasher.finalize());

        assert_eq!(
            recorded_hash, current_hash,
            "project_io.proto has changed since project_io.gen.rs was generated.\n\
             Run `pnpm build:gen-protobufs` to regenerate the Rust protobuf code."
        );
    }
}

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
