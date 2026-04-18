// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.
//
// ---------------------------------------------------------------------
// Portions of this file are adapted from rapidhash V3 and retain their
// upstream MIT license.  The original rapidhash is:
//
//   Copyright (C) 2025 Nicolas De Carli
//   based on 'wyhash' by Wang Yi
//   https://github.com/Nicoshev/rapidhash
//
// and the Go reference port used for cross-validation is:
//
//   Copyright (C) 2025 Bobby Powers
//   third_party/go-rapidhash
//
// Both are MIT-licensed (see their LICENSE files):
//
// Permission is hereby granted, free of charge, to any person obtaining a
// copy of this software and associated documentation files (the "Software"),
// to deal in the Software without restriction, including without limitation
// the rights to use, copy, modify, merge, publish, distribute, sublicense,
// and/or sell copies of the Software, and to permit persons to whom the
// Software is furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included
// in all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
// MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
// IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
// TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
// SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

//! Targeted port of the rapidhash V3 `HashMicro` variant.
//!
//! rapidhash is a fast, high-quality 64-bit non-cryptographic hash derived
//! from Wang Yi's wyhash.  We port only the `HashMicro` variant because
//! our call site in `ltm.rs` feeds it sorted `u32` circuit fingerprints
//! in the 2..=80 element range (8..320 bytes, mean ~188 bytes) and
//! `HashMicro`'s 80-byte unrolled inner loop is an excellent match for
//! that size distribution (the general `Hash` variant's 112-byte unroll
//! and 224-byte mega-loop pay for themselves only above ~1 KB, which we
//! never see; `HashNano` loses throughput above 48 bytes).
//!
//! Determinism is a hard requirement: the caller stores the returned
//! fingerprint in a `HashSet<u64>` that is later used as a salsa cache
//! key, so the hash for a given input must be identical across runs.
//! We use the default rapidhash V3 secret and a caller-supplied fixed
//! seed, and we assume little-endian byte order (enforced by a
//! compile-time check below -- our targets are x86-64, aarch64, and
//! wasm32, all little-endian).
//!
//! The core algorithm reads 8 bytes at a time via `u64::from_le_bytes`,
//! which LLVM lowers to a single `mov` on little-endian hosts and gives
//! us safe unaligned reads for free.
//!
//! # Correctness
//!
//! The byte-level output is cross-validated against the Go reference
//! port (`third_party/go-rapidhash`) for a fixed corpus of inputs
//! spanning all branches of the length-dispatch cascade.  Those vectors
//! are baked into the module-level tests below.

// The crate is compiled with `#![deny(unsafe_code)]`. We opt in to a
// single narrowly-scoped `unsafe` call below (slice re-interpretation of
// `&[u32]` as `&[u8]`) with an explicit SAFETY comment.  Everything else
// is safe Rust.
#![allow(unsafe_code)]

// Compile-time guard: the rapidhash V3 reference implementations assume
// little-endian byte order for their 8-byte / 4-byte reads, and the Go
// cross-validation vectors were generated in that mode.  Running on a
// big-endian host would silently produce different hash values and
// invalidate cached `LoopCircuitsResult`s, so we refuse to compile.
#[cfg(not(target_endian = "little"))]
compile_error!(
    "simlin-engine::rapidhash requires a little-endian target: \
     the hash output is intentionally byte-order-dependent so salsa \
     cache keys stay stable across runs."
);

/// Default 8x64-bit secret from rapidhash V3 (`rapid_secret` in
/// `third_party/rapidhash/rapidhash.h`).  Hard-coded rather than derived
/// so the Rust and Go implementations produce identical hashes.
const DEFAULT_SECRET: [u64; 8] = [
    0x2d358dccaa6c78a5,
    0x8bb84b93962eacc9,
    0x4b33a62ed433d4a3,
    0x4d5a2da51de1aa47,
    0xa0761d6478bd642f,
    0xe7037ed1a0b428db,
    0x90ed1765281c388c,
    0xaaaaaaaaaaaaaaaa,
];

/// 64x64 -> 128 multiply returning `(low, high)`.
///
/// On x86-64 and aarch64 the `u128` path compiles to a single `mulq`
/// / `umulh`+`mul` pair -- no stack spill, no slow multi-limb expansion.
/// Forced-inlined because the hot loop calls this ~10x per 80-byte
/// block; letting a `callq` remain here stalls the in-flight mul chain.
#[inline(always)]
fn rapid_mum(a: u64, b: u64) -> (u64, u64) {
    let r = (a as u128).wrapping_mul(b as u128);
    (r as u64, (r >> 64) as u64)
}

/// Multiply-and-xor mix: returns the XOR of the high and low 64 bits
/// of `a * b`.  This is the lone non-linear primitive of rapidhash; all
/// other operations are XORs and loads.
#[inline(always)]
fn rapid_mix(a: u64, b: u64) -> u64 {
    let (lo, hi) = rapid_mum(a, b);
    lo ^ hi
}

/// Read 8 little-endian bytes from the first 8 bytes of `block`.
///
/// Callers pass in a typed chunk slice (e.g. `&[u8; 80]`) so LLVM
/// knows statically that the read is in bounds -- no runtime check is
/// emitted.  The `try_into().unwrap()` on a known-length array slice
/// collapses to a direct unaligned 8-byte load on x86-64 / aarch64.
/// Using `copy_from_slice` instead generates a `memcpy`-style callq.
#[inline(always)]
fn read64_at<const N: usize>(block: &[u8; N], off: usize) -> u64 {
    let chunk: [u8; 8] = block[off..off + 8].try_into().unwrap();
    u64::from_le_bytes(chunk)
}

/// Read 4 little-endian bytes from `p` at offset `off`.
///
/// Used only on the <=16-byte path where the input slice length is not
/// fixed at compile time, so we can't template over `N`.  The caller
/// guarantees `off + 4 <= p.len()`.
#[inline(always)]
fn read32_dyn(p: &[u8], off: usize) -> u64 {
    let chunk: [u8; 4] = p[off..off + 4].try_into().unwrap();
    u32::from_le_bytes(chunk) as u64
}

/// Read 8 little-endian bytes from `p` at offset `off` (dynamic-length
/// slice form for the <=16-byte path and the finalization tail read).
#[inline(always)]
fn read64_dyn(p: &[u8], off: usize) -> u64 {
    let chunk: [u8; 8] = p[off..off + 8].try_into().unwrap();
    u64::from_le_bytes(chunk)
}

/// `HashMicro`-variant byte hash.
///
/// Faithful port of `rapidhashMicro_internal` in
/// `third_party/rapidhash/rapidhash.h` using the default rapid secret.
/// The control flow, lane ordering, and final mix exactly mirror the
/// C/Go reference; the only differences are Rust idioms (slice reads,
/// branching by match, `u128`-backed multiply).
///
/// # Determinism
///
/// For any `(key, seed)` pair the output is byte-for-byte stable across
/// runs and matches the Go port's `HashMicro(key, seed)`.
#[inline]
pub fn hash_bytes(key: &[u8], seed: u64) -> u64 {
    let len = key.len();
    let len_u64 = len as u64;
    let mut seed = seed ^ rapid_mix(seed ^ DEFAULT_SECRET[2], DEFAULT_SECRET[1]);

    // `i` starts at the full length and is decremented inside the big-input
    // branch as whole 80-byte blocks are consumed.  The final mix folds
    // `i` (not the original `len`) into `b ^ secret[1] ^ i` -- in the
    // <= 16-byte branch `i == len` so the two are identical, but in the
    // large-input branch `i` ends as the 0..=80 residual byte count and
    // the hash would mismatch the reference if we substituted `len`.
    let mut i = len_u64;

    let (a, b) = if len <= 16 {
        if len >= 4 {
            // The `seed ^= len` mutation is part of the rapidhash spec
            // for 4..=16-byte inputs: it folds the length into the
            // entropy pool before the tail mix.  It is not a bug.
            seed ^= len_u64;
            if len >= 8 {
                (read64_dyn(key, 0), read64_dyn(key, len - 8))
            } else {
                (read32_dyn(key, 0), read32_dyn(key, len - 4))
            }
        } else if len > 0 {
            (
                ((key[0] as u64) << 45) | (key[len - 1] as u64),
                key[len >> 1] as u64,
            )
        } else {
            (0u64, 0u64)
        }
    } else {
        let mut off = 0usize;

        if i > 80 {
            // Four parallel accumulator lanes beyond `seed`. Splitting
            // the mixing state into independent chains is what lets the
            // out-of-order pipeline run the mul/xor sequence at one
            // block per ~5 cycles instead of serializing on a single
            // dependency chain.
            let mut see1 = seed;
            let mut see2 = seed;
            let mut see3 = seed;
            let mut see4 = seed;

            // Each iteration consumes 80 bytes = 10 * u64 reads = 5
            // parallel `rapid_mix` calls.  We split an 80-byte typed
            // block out of `key` so every subsequent `read64_at` sees a
            // slice whose length is a compile-time constant -- without
            // this, LLVM emits a full slice bounds check per read and
            // the loop runs ~2x slower on x86-64.
            while i > 80 {
                let block: &[u8; 80] = key[off..off + 80]
                    .try_into()
                    .expect("bounds-checked by i > 80");
                seed = rapid_mix(
                    read64_at(block, 0) ^ DEFAULT_SECRET[0],
                    read64_at(block, 8) ^ seed,
                );
                see1 = rapid_mix(
                    read64_at(block, 16) ^ DEFAULT_SECRET[1],
                    read64_at(block, 24) ^ see1,
                );
                see2 = rapid_mix(
                    read64_at(block, 32) ^ DEFAULT_SECRET[2],
                    read64_at(block, 40) ^ see2,
                );
                see3 = rapid_mix(
                    read64_at(block, 48) ^ DEFAULT_SECRET[3],
                    read64_at(block, 56) ^ see3,
                );
                see4 = rapid_mix(
                    read64_at(block, 64) ^ DEFAULT_SECRET[4],
                    read64_at(block, 72) ^ see4,
                );
                off += 80;
                i -= 80;
            }
            // `i` is now the 0..=80-byte residual; the tail drain reads
            // from `off` and the final 16 bytes are read from the end
            // of the original buffer.

            // Collapse lanes pairwise into `seed`.  The fold order
            // matches the reference implementation byte-for-byte.
            seed ^= see1;
            see2 ^= see3;
            seed ^= see4;
            seed ^= see2;
        }

        // Drain the 17..=80-byte tail with 0..=3 serial mixes.  Each
        // `if i > N` nest reads up to byte (N, N+16] from `off`, so we
        // take a fixed-size &[u8; K] block per level; the `try_into`
        // collapses to a length check that LLVM can propagate through
        // all 4 / 8 / 12 / 16 subsequent `read64_at` calls.  The old
        // dynamic-index form (`key[off..off+8]`) re-checked bounds on
        // every read and roughly doubled the hash's runtime on our
        // inputs.
        if i > 16 {
            let tail16: &[u8; 16] = key[off..off + 16]
                .try_into()
                .expect("i > 16 implies at least 16 bytes remain");
            seed = rapid_mix(
                read64_at(tail16, 0) ^ DEFAULT_SECRET[2],
                read64_at(tail16, 8) ^ seed,
            );
            if i > 32 {
                let tail32: &[u8; 32] = key[off..off + 32]
                    .try_into()
                    .expect("i > 32 implies at least 32 bytes remain");
                seed = rapid_mix(
                    read64_at(tail32, 16) ^ DEFAULT_SECRET[2],
                    read64_at(tail32, 24) ^ seed,
                );
                if i > 48 {
                    let tail48: &[u8; 48] = key[off..off + 48]
                        .try_into()
                        .expect("i > 48 implies at least 48 bytes remain");
                    seed = rapid_mix(
                        read64_at(tail48, 32) ^ DEFAULT_SECRET[1],
                        read64_at(tail48, 40) ^ seed,
                    );
                    if i > 64 {
                        let tail64: &[u8; 64] = key[off..off + 64]
                            .try_into()
                            .expect("i > 64 implies at least 64 bytes remain");
                        seed = rapid_mix(
                            read64_at(tail64, 48) ^ DEFAULT_SECRET[1],
                            read64_at(tail64, 56) ^ seed,
                        );
                    }
                }
            }
        }

        // Final 16 bytes are always taken from the tail of the original
        // buffer so single-bit flips in any byte affect the output (the
        // interior lane mixes cover middle bytes, the tail ensures the
        // final two u64s are included regardless of `i`).
        let a = read64_dyn(key, len - 16) ^ i;
        let b = read64_dyn(key, len - 8);
        (a, b)
    };

    // Finalization: reduce `a`, `b`, `seed`, and the residual `i` into a
    // single u64.  The `a ^ secret[7]` / `b ^ secret[1] ^ i` avalanche
    // stirs in the remaining entropy and length residue.  In the small-
    // input branch `i` was never decremented so it still equals `len_u64`,
    // which matches the reference behavior.  The `rapid_mum` before
    // `rapid_mix` is arithmetically redundant with the next mix but is
    // part of the specified finalization, so we keep it byte-for-byte.
    let a = a ^ DEFAULT_SECRET[1];
    let b = b ^ seed;
    let (a, b) = rapid_mum(a, b);
    rapid_mix(a ^ DEFAULT_SECRET[7], b ^ DEFAULT_SECRET[1] ^ i)
}

/// Convenience wrapper for the LTM callsite: hash the little-endian byte
/// representation of a `&[u32]`.
///
/// This is the primary entry point used from `ltm.rs::johnson_circuit`
/// to fingerprint sorted vertex sets of discovered cycles.  On every
/// target we support (x86-64 Linux/macOS, aarch64 Linux/macOS, wasm32)
/// `u32` values are already stored little-endian, so
/// re-interpreting the slice as raw bytes is equivalent to a logical
/// `u32::to_le_bytes` on each element without the memcpy.
#[inline]
pub fn hash_u32_slice(vals: &[u32], seed: u64) -> u64 {
    // SAFETY:
    //   * `vals.as_ptr()` is valid for reads of `vals.len() * 4` bytes,
    //     because a `&[u32]` guarantees contiguous storage of that
    //     many initialized bytes.
    //   * Every bit pattern of `u8` is valid, so reinterpreting the
    //     storage as `&[u8]` is sound regardless of the original
    //     `u32` values.
    //   * The produced `&[u8]` does not outlive the input slice -- we
    //     only pass it into `hash_bytes`, which is itself bounded by
    //     the caller's borrow.
    //   * Alignment: u32 has alignment 4, but `*const u8` only requires
    //     alignment 1, so the downcast is trivially aligned.
    //   * `vals.len() * 4` cannot overflow: a slice's size in bytes is
    //     constrained by `isize::MAX` per Rust's pointer aliasing
    //     rules, which bounds `vals.len() <= isize::MAX / 4`.
    //   * Endianness: enforced little-endian at crate level via the
    //     `compile_error!` guard above, so the byte sequence matches
    //     what a caller would get from flattening
    //     `u32::to_le_bytes` per element.
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(vals.as_ptr() as *const u8, vals.len() * 4) };
    hash_bytes(bytes, seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // Stability / self-consistency
    // ---------------------------------------------------------------

    #[test]
    fn empty_input_is_stable() {
        // Expected values mirror the Go port's test vector for
        // HashMicro(len=0, seed=0).
        assert_eq!(hash_bytes(&[], 0), 0x0338dc4be2cecdae);
        assert_eq!(hash_bytes(&[], 0), hash_bytes(&[], 0));
    }

    #[test]
    fn hashes_are_deterministic_across_calls() {
        // Cover every branch of the length cascade.
        let sizes: [usize; 13] = [1, 2, 3, 4, 7, 8, 15, 16, 17, 80, 81, 188, 320];
        for &n in &sizes {
            let buf: Vec<u8> = (0..n).map(|i| (i as u8).wrapping_mul(7)).collect();
            let h1 = hash_bytes(&buf, 0xabcdef0123456789);
            let h2 = hash_bytes(&buf, 0xabcdef0123456789);
            assert_eq!(h1, h2, "hash not stable for len={n}");
            // Different seed must change the hash (collision at 2^-64
            // is negligible for a single pair).
            let h3 = hash_bytes(&buf, 0xabcdef0123456788);
            assert_ne!(h1, h3, "seed did not affect hash for len={n}");
        }
    }

    #[test]
    fn u32_and_bytes_roundtrip_match() {
        // Whatever the `hash_u32_slice` wrapper feeds into `hash_bytes`
        // must match a manual flatten via `u32::to_le_bytes`.
        let vals: Vec<u32> = (0u32..47).map(|i| 7 + 13 * i).collect();
        let mut flat = Vec::with_capacity(vals.len() * 4);
        for &v in &vals {
            flat.extend_from_slice(&v.to_le_bytes());
        }
        let seed = 0xabcdef0123456789u64;
        assert_eq!(hash_u32_slice(&vals, seed), hash_bytes(&flat, seed));
    }

    #[test]
    fn u32_slice_wrapper_handles_empty() {
        // `&[u32]` of length 0 must produce the same hash as an empty
        // byte slice -- both are "no bytes" inputs.
        let empty: &[u32] = &[];
        assert_eq!(
            hash_u32_slice(empty, 0xabcdef0123456789),
            hash_bytes(&[], 0xabcdef0123456789),
        );
    }

    // ---------------------------------------------------------------
    // Cross-validation vs. Go reference port.
    //
    // These vectors were generated by running the Go `HashMicro`
    // implementation in `third_party/go-rapidhash` on the exact inputs
    // below (see the commit message for the harness).  A mismatch here
    // means the Rust port has diverged from the reference: do not
    // "fix" it by changing the expected value without re-running the
    // Go generator.
    // ---------------------------------------------------------------

    /// Mirror of the Go helper's `makeBuffer`: a Knuth-style LCG with
    /// the same constants that feeds `byte(state >> 33)` per step so
    /// Rust and Go produce byte-identical buffers from the same seed.
    fn lcg_buffer(n: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.push((state >> 33) as u8);
        }
        out
    }

    fn check_vec(input: &[u8], seed: u64, expected: u64, label: &str) {
        let got = hash_bytes(input, seed);
        assert_eq!(
            got,
            expected,
            "{label}: len={} seed={:#x} expected={:#x} got={:#x}",
            input.len(),
            seed,
            expected,
            got,
        );
    }

    #[test]
    fn cross_validate_go_vectors_small() {
        // Tiny inputs (<= 16 bytes): all three Go variants produce the
        // same values, so these also double-check we picked the right
        // low-length branch.
        check_vec(&[], 0, 0x0338dc4be2cecdae, "empty/s=0");
        check_vec(&[], 1, 0xad700ecdf353d5ca, "empty/s=1");
        check_vec(&[], 0xabcdef0123456789, 0xf5e4283856d3700b, "empty/sSim");
        check_vec(&[0x42], 0, 0xdf39ad7f42b5c997, "1b");
        check_vec(&[0x01, 0x02, 0x03], 0, 0x1adbce663d9a75a6, "3b");
        check_vec(&[0xde, 0xad, 0xbe, 0xef], 0, 0x3faf8cd66e874f03, "4b");
        check_vec(
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07],
            0,
            0x882c9ab41aa6037a,
            "7b",
        );
        check_vec(
            &[0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77],
            0,
            0xe4c6bfcfaf1f80d1,
            "8b",
        );
        check_vec(
            &[
                0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
                0x0e,
            ],
            0,
            0x8ec6dfea933104bb,
            "15b",
        );
        check_vec(
            &[
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ],
            0,
            0x1a32399179c20bd9,
            "16b",
        );
        check_vec(
            &[
                0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d, 0x6e, 0x6f, 0x70, 0x71,
                0x72, 0x73, 0x74,
            ],
            0,
            0xe2416cd4ea7bf627,
            "17b",
        );
    }

    #[test]
    fn cross_validate_go_vectors_medium_and_large() {
        // Each entry: (len, go_seed_arg, expected_hash, label).
        // The buffer is generated by `lcg_buffer(len, 0xcafebabedeadbeef + len)`
        // which mirrors the helper in `tmp/rapidhash-vectors/main.go`.
        let cases: &[(usize, u64, u64, &str)] = &[
            (32, 0, 0xbb5ea522d9542c20, "lcg-32"),
            (32, 0xabcdef0123456789, 0x40d571bd2796812c, "lcg-32-sSim"),
            (64, 0, 0x2e51fee0460d578e, "lcg-64"),
            (64, 0xabcdef0123456789, 0x2411fb1c17179b87, "lcg-64-sSim"),
            (80, 0, 0x24ffd6329bc85560, "lcg-80"),
            (80, 0xabcdef0123456789, 0xb6b43e4361bc51f5, "lcg-80-sSim"),
            (81, 0, 0xd283841d362feb50, "lcg-81"),
            (81, 0xabcdef0123456789, 0x86c57b5e9ba9e23c, "lcg-81-sSim"),
            (112, 0, 0x6e5e5d3c246ef77c, "lcg-112"),
            (112, 0xabcdef0123456789, 0xb813e805b03921d3, "lcg-112-sSim"),
            (113, 0, 0x8e2c3315965051dc, "lcg-113"),
            (113, 0xabcdef0123456789, 0xe9f81ac5fb3cd2f0, "lcg-113-sSim"),
            (128, 0, 0x431a517fbf803d4f, "lcg-128"),
            (128, 0xabcdef0123456789, 0x1d16f322de3fb7b0, "lcg-128-sSim"),
            (188, 0, 0xe9cf0dc52eb51f94, "lcg-188"),
            (188, 0xabcdef0123456789, 0x92595f0113b5ed45, "lcg-188-sSim"),
            (256, 0, 0xf5439e104fb80782, "lcg-256"),
            (256, 0xabcdef0123456789, 0x8f15bec5bf5dcaa7, "lcg-256-sSim"),
            (320, 0, 0x5decab8ff49b9b8a, "lcg-320"),
            (320, 0xabcdef0123456789, 0xb588bda4b626826a, "lcg-320-sSim"),
            (321, 0, 0xd48ef9fab69d9034, "lcg-321"),
            (321, 0xabcdef0123456789, 0xc974b3f67e459c9b, "lcg-321-sSim"),
            (1024, 0, 0x94614af768e41862, "lcg-1024"),
            (
                1024,
                0xabcdef0123456789,
                0xb836b6c3c94fe09c,
                "lcg-1024-sSim",
            ),
        ];
        for &(n, seed, expected, label) in cases {
            let lcg_seed = 0xcafebabedeadbeefu64.wrapping_add(n as u64);
            let buf = lcg_buffer(n, lcg_seed);
            check_vec(&buf, seed, expected, label);
        }
    }

    #[test]
    fn cross_validate_zero_buffers() {
        // All-zero buffers catch bugs where the state is swallowed by a
        // leading zero but the tail-read recovers it -- a common failure
        // mode when porting length-dependent mixes.
        check_vec(&[0u8; 80], 0, 0xb4e056afb71930da, "zeros-80");
        check_vec(&[0u8; 188], 0, 0xea799a86d07f38e9, "zeros-188");
        check_vec(&[0u8; 320], 0, 0x75c7bc181a164136, "zeros-320");
    }

    #[test]
    fn cross_validate_u32_slice() {
        // A representative sorted-index input for our callsite (47
        // u32s, strided); ensures the u32-slice wrapper byte-matches
        // the Go output on an LTM-shaped vector.
        let vals: Vec<u32> = (0u32..47).map(|i| 7 + 13 * i).collect();
        assert_eq!(vals.len() * 4, 188);
        let got = hash_u32_slice(&vals, 0xabcdef0123456789);
        assert_eq!(got, 0xed024dfa5a5c4344, "u32-47 slice");
    }
}
