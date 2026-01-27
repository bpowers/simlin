// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Profiling harness for open_vensim vs open_vensim_native on the C-LEARN model.
//!
//! Usage:
//!   cargo build --example profile_clearn --features xmutil --release
//!   # then run under perf, valgrind, etc:
//!   perf stat ./target/release/examples/profile_clearn xmutil
//!   perf stat ./target/release/examples/profile_clearn native
//!   valgrind --tool=dhat ./target/release/examples/profile_clearn xmutil
//!   valgrind --tool=dhat ./target/release/examples/profile_clearn native

use std::io::BufReader;

use simlin_compat::{open_vensim, open_vensim_native};

const CLEARN_MDL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl"
);

const ITERATIONS: usize = 10;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("both");

    let mdl_contents = std::fs::read(CLEARN_MDL).expect("failed to read C-LEARN model");

    match mode {
        "xmutil" => {
            for _ in 0..ITERATIONS {
                let mut reader = BufReader::new(mdl_contents.as_slice());
                let _project = open_vensim(&mut reader).expect("open_vensim should succeed");
            }
        }
        "native" => {
            for _ in 0..ITERATIONS {
                let mut reader = BufReader::new(mdl_contents.as_slice());
                let _project =
                    open_vensim_native(&mut reader).expect("open_vensim_native should succeed");
            }
        }
        "both" => {
            eprintln!("--- xmutil path ---");
            for _ in 0..ITERATIONS {
                let mut reader = BufReader::new(mdl_contents.as_slice());
                let _project = open_vensim(&mut reader).expect("open_vensim should succeed");
            }
            eprintln!("--- native path ---");
            for _ in 0..ITERATIONS {
                let mut reader = BufReader::new(mdl_contents.as_slice());
                let _project =
                    open_vensim_native(&mut reader).expect("open_vensim_native should succeed");
            }
        }
        _ => {
            eprintln!("Usage: profile_clearn [xmutil|native|both]");
            std::process::exit(1);
        }
    }
}
