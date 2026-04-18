// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Throughput benchmark for the LTM circuit fingerprint hash.
//!
//! Compares the current `rapidhash::hash_u32_slice` (rapidhash V3
//! `HashMicro`) against the original FNV-1a baseline at a range of input
//! sizes covering the LTM call site's observed distribution
//! (8..=320 bytes, ~188 bytes mean, ~47 u32 elements) and one large
//! size (1 KiB) to sanity-check behavior outside the hot range.
//!
//! Run with `cargo bench -p simlin-engine --bench rapidhash_bench`.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use simlin_engine::rapidhash::{hash_bytes, hash_u32_slice};

/// Reference FNV-1a 64-bit hash, u32-at-a-time.
///
/// This is a verbatim copy of the pre-rapidhash implementation that
/// lived in `src/simlin-engine/src/ltm.rs` -- keep it here as the apples-
/// to-apples baseline so the criterion diff reflects a real replacement
/// and not a comparison against some other hash crate's tuning.
fn fnv1a_u32_slice(vals: &[u32]) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut h: u64 = FNV_OFFSET_BASIS;
    for &v in vals {
        h ^= v as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Input sizes in bytes.  Spans the 8..=320 byte sweet spot plus an
/// outlier size so we can tell if `HashMicro` loses ground for large
/// keys (it shouldn't for these sizes, but it's a cheap guardrail).
const SIZES_BYTES: &[usize] = &[8, 32, 64, 128, 188, 256, 320, 1024];

/// Build a `Vec<u32>` of the requested byte length by striding a simple
/// pseudo-random sequence -- realistic enough that the optimizer can't
/// fold it to a constant, deterministic enough that benchmark runs are
/// comparable.
fn make_u32_input(size_bytes: usize) -> Vec<u32> {
    assert!(size_bytes.is_multiple_of(4), "u32 input must be 4-aligned");
    let n = size_bytes / 4;
    let mut out = Vec::with_capacity(n);
    let mut state: u64 = 0xdeadbeef_cafebabe;
    for _ in 0..n {
        // Xorshift64 -- two xors and two shifts, cheap and good enough
        // to defeat loop-invariant code motion.
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(state as u32);
    }
    out
}

fn bench_hashes(c: &mut Criterion) {
    let seed = 0xabcdef0123456789u64;

    // rapidhash_bench vs fnv1a_bench as separate groups so criterion's
    // throughput display is unambiguous.
    let mut rapid_group = c.benchmark_group("rapidhash_micro");
    for &size in SIZES_BYTES {
        let input = make_u32_input(size);
        rapid_group.throughput(Throughput::Bytes(size as u64));
        rapid_group.bench_with_input(BenchmarkId::from_parameter(size), &input, |b, input| {
            b.iter(|| black_box(hash_u32_slice(black_box(input), seed)));
        });
    }
    rapid_group.finish();

    let mut fnv_group = c.benchmark_group("fnv1a_u32");
    for &size in SIZES_BYTES {
        let input = make_u32_input(size);
        fnv_group.throughput(Throughput::Bytes(size as u64));
        fnv_group.bench_with_input(BenchmarkId::from_parameter(size), &input, |b, input| {
            b.iter(|| black_box(fnv1a_u32_slice(black_box(input))));
        });
    }
    fnv_group.finish();

    // Byte-level hash: also relevant since the `hash_u32_slice` wrapper
    // adds a single `from_raw_parts` call that we want to verify has
    // zero measurable cost.
    let mut raw_group = c.benchmark_group("rapidhash_micro_bytes");
    for &size in SIZES_BYTES {
        let input: Vec<u8> = make_u32_input(size)
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        raw_group.throughput(Throughput::Bytes(size as u64));
        raw_group.bench_with_input(BenchmarkId::from_parameter(size), &input, |b, input| {
            b.iter(|| black_box(hash_bytes(black_box(input), seed)));
        });
    }
    raw_group.finish();
}

criterion_group!(benches, bench_hashes);
criterion_main!(benches);
