// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The files a model is kept in, read and written: Vensim MDL and VDF, XMILE,
//! the systems format, JSON (native and SD-AI), and the protobuf a stored
//! project is.

#![deny(unsafe_code)]

// Re-exported so downstream crates (libsimlin, simlin-cli) encode and decode
// `project_io` messages through the same runtime the generated code was built
// against, instead of each declaring a `buffa` version of its own that has to
// track this crate's.
pub use buffa;

pub mod compat;
pub mod data_provider;
pub mod element_equations;
mod import_losses;
pub mod io;
pub mod json;
#[cfg(test)]
mod json_proptest;
pub mod json_sdai;
#[cfg(test)]
mod json_sdai_proptest;
pub mod mdl;
// Generated code: silence the lints buffa-codegen's output trips rather than
// hand-editing a file the freshness test regenerates.
#[allow(
    clippy::derive_partial_eq_without_eq,
    clippy::derivable_impls,
    clippy::match_single_binding
)]
#[path = "project_io.gen.rs"]
pub mod project_io;
pub mod serde;
#[cfg(test)]
mod serde_proptest;
pub mod systems;
#[cfg(test)]
#[path = "systems_stdlib_tests.rs"]
mod systems_stdlib_tests;
#[cfg(test)]
mod test_sir_xmile;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod vdf;
pub mod xmile;

pub use self::import_losses::ImportWarning;

// Re-export compat functions at the crate root for convenience
#[cfg(any(test, feature = "file_io"))]
pub use self::compat::{load_csv, load_dat};
pub use self::compat::{
    open_systems, open_vensim, open_vensim_with_data, open_vensim_with_data_and_warnings,
    open_vensim_with_warnings, open_xmile, open_xmile_with_warnings, to_mdl, to_mdl_with_warnings,
    to_systems, to_xmile,
};
#[cfg(any(test, feature = "file_io"))]
pub use self::data_provider::FilesystemDataProvider;
pub use self::data_provider::{DataProvider, NullDataProvider};

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
