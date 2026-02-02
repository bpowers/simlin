// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Profiling harness for open_vensim_xmutil vs open_vensim on the C-LEARN model.
//!
//! Usage:
//!   cargo build --example profile_clearn --features xmutil --release
//!   # then run under perf, valgrind, etc:
//!   perf stat ./target/release/examples/profile_clearn xmutil
//!   perf stat ./target/release/examples/profile_clearn native
//!   valgrind --tool=dhat ./target/release/examples/profile_clearn xmutil
//!   valgrind --tool=dhat ./target/release/examples/profile_clearn native

use simlin_compat::{open_vensim, open_vensim_xmutil};

const CLEARN_MDL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl"
);

const ITERATIONS: usize = 10;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("both");

    let mdl_contents = std::fs::read_to_string(CLEARN_MDL).expect("failed to read C-LEARN model");

    match mode {
        "xmutil" => {
            for _ in 0..ITERATIONS {
                let _project =
                    open_vensim_xmutil(&mdl_contents).expect("open_vensim_xmutil should succeed");
            }
        }
        "native" => {
            for _ in 0..ITERATIONS {
                let _project = open_vensim(&mdl_contents).expect("open_vensim should succeed");
            }
        }
        "both" => {
            eprintln!("--- xmutil path ---");
            for _ in 0..ITERATIONS {
                let _project =
                    open_vensim_xmutil(&mdl_contents).expect("open_vensim_xmutil should succeed");
            }
            eprintln!("--- native path ---");
            for _ in 0..ITERATIONS {
                let _project = open_vensim(&mdl_contents).expect("open_vensim should succeed");
            }
        }
        _ => {
            eprintln!("Usage: profile_clearn [xmutil|native|both]");
            std::process::exit(1);
        }
    }
}
