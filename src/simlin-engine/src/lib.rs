// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

pub use prost;

mod ast;
pub mod builtins;
mod builtins_visitor;
pub mod common;
mod compiler;
pub mod datamodel;
mod dimensions;
pub mod json;
pub mod json_sdai;
mod lexer;
mod model;
mod parser;
mod patch;
#[allow(clippy::derive_partial_eq_without_eq)]
#[path = "project_io.gen.rs"]
pub mod project_io;
pub mod serde;
mod variable;
mod stdlib {
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}
#[cfg(feature = "ai_info")]
pub mod ai_info;
#[cfg(test)]
mod array_tests;
mod bytecode;
pub mod interpreter;
#[cfg(test)]
mod json_proptest;
#[cfg(test)]
mod json_sdai_proptest;
pub mod ltm;
pub mod ltm_augment;
mod project;
pub mod test_common;
#[cfg(test)]
mod testutils;
#[cfg(test)]
mod unit_checking_test;
mod units;
mod units_check;
mod units_infer;
mod vm;

pub use self::common::{Error, ErrorCode, Result, canonicalize};
pub use self::interpreter::Simulation;
pub use self::model::{ModelStage1, resolve_non_private_dependencies};
pub use self::patch::apply_patch;
pub use self::project::Project;
pub use self::variable::{Variable, identifier_set};
pub use self::vm::Vm;
// Re-export results types from simlin-core
pub use simlin_core::{Method, Results, Specs as SimSpecs};

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
            .expect("failed to read project_io.gen.rs - run `yarn build:gen-protobufs`");

        let recorded_hash = extract_hash_from_gen_file(&gen_content)
            .expect("project_io.gen.rs is missing SHA256 hash header");

        let proto_content = fs::read(PROTO_FILE).expect("failed to read project_io.proto");
        let mut hasher = Sha256::new();
        hasher.update(&proto_content);
        let current_hash = format!("{:x}", hasher.finalize());

        assert_eq!(
            recorded_hash, current_hash,
            "project_io.proto has changed since project_io.gen.rs was generated.\n\
             Run `yarn build:gen-protobufs` to regenerate the Rust protobuf code."
        );
    }
}
