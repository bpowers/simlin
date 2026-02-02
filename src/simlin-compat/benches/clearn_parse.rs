// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Benchmarks comparing open_vensim_xmutil (xmutil C++) vs open_vensim (pure Rust)
//! for parsing the C-LEARN v77 model.

use criterion::{Criterion, criterion_group, criterion_main};

use simlin_compat::{open_vensim, open_vensim_xmutil};

const CLEARN_MDL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl"
);

fn bench_open_vensim_xmutil(c: &mut Criterion) {
    let mdl_contents = std::fs::read_to_string(CLEARN_MDL).expect("failed to read C-LEARN model");

    c.bench_function("open_vensim_xmutil/clearn", |b| {
        b.iter(|| open_vensim_xmutil(&mdl_contents).expect("open_vensim_xmutil should succeed"));
    });
}

fn bench_open_vensim(c: &mut Criterion) {
    let mdl_contents = std::fs::read_to_string(CLEARN_MDL).expect("failed to read C-LEARN model");

    c.bench_function("open_vensim/clearn", |b| {
        b.iter(|| open_vensim(&mdl_contents).expect("open_vensim should succeed"));
    });
}

criterion_group!(benches, bench_open_vensim_xmutil, bench_open_vensim,);
criterion_main!(benches);
