// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

//! Content hashing for echo suppression on the file watcher's ingestion path.
//!
//! When the save handler atomic-writes a file, the OS watcher (Phase 4) sees
//! the resulting `Modify`/`Create` event moments later. Without a stable
//! content fingerprint we'd round-trip those bytes back through the merge
//! primitive — wasted work in the best case, a feedback loop in the worst.
//! `content_hash` lets the registry remember the bytes it just wrote so the
//! watcher can short-circuit when the disk content matches.
//!
//! XXH3-64 is chosen for: (a) speed (multi-GB/s on modern x86_64), (b) zero
//! allocations (`oneshot` operates on the input slice directly), (c) pure-Rust
//! implementation, and (d) widespread deployment (twox-hash sees 5M+
//! downloads/month). Cryptographic strength is not a requirement — a
//! collision only causes a missed echo-suppression, which results in a
//! redundant merge, not a correctness violation.

use twox_hash::XxHash3_64;

/// Compute a 64-bit content hash over `bytes`. Stable across runs and across
/// machines (the underlying XXH3-64 implementation is deterministic).
pub fn content_hash(bytes: &[u8]) -> u64 {
    XxHash3_64::oneshot(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured constant from a known-good run (twox-hash 2.x). Pinning the
    /// exact value (rather than merely "the function is deterministic")
    /// catches an upstream algorithm change, which would silently break
    /// echo suppression for every file written before the upgrade.
    const HASH_OF_HELLO: u64 = 0x9555_e855_5c62_dcfd;

    #[test]
    fn hash_of_hello_matches_captured_constant() {
        assert_eq!(content_hash(b"hello"), HASH_OF_HELLO);
    }
}
