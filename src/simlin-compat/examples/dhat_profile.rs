// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! DHAT allocation profiler for open_vensim on the C-LEARN model.
//!
//! Usage:
//!   cargo run --example dhat_profile --release
//!
//! This produces a dhat-heap.json file that can be viewed at:
//!   https://nnethercote.github.io/dh_view/dh_view.html

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use simlin_compat::open_vensim;

const CLEARN_MDL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl"
);

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    let mdl_contents = std::fs::read_to_string(CLEARN_MDL).expect("failed to read C-LEARN model");

    let _project = open_vensim(&mdl_contents).expect("open_vensim should succeed");

    // dhat stats are printed on drop
}
