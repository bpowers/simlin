// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Single integration-test harness for simlin-engine.
//!
//! All integration tests are modules of this one binary rather than separate
//! `tests/*.rs` files. Each top-level test file becomes its own ~100MB binary
//! that statically links the full dependency graph, and on macOS every fresh
//! binary pays a serialized first-exec security scan (~1-3s each), which blew
//! the pre-commit `cargo test` wall-clock budget. One harness per crate keeps
//! link time, disk, and scan cost constant as tests grow. See GH issue #706.
//!
//! Add new integration tests as a `mod` here, not as a new file directly
//! under `tests/`. The one exception is `tests/vm_alloc.rs`, which installs a
//! counting `#[global_allocator]` and therefore must remain its own process.
//!
//! Feature gating: modules that exercise `file_io`- or `xmutil`-gated engine
//! APIs are `#[cfg]`-gated below. These used to be `required-features`
//! entries in Cargo.toml; a `cfg`-gated module compiles to nothing when the
//! feature is off, which has the same effect (the tests only exist when the
//! feature is enabled) without skipping the whole harness.

// The shared comparison helpers. Most of their consumers are file_io-gated,
// so a build without file_io legitimately leaves parts of the module unused.
#[cfg_attr(not(feature = "file_io"), allow(dead_code))]
mod test_helpers;

mod clearn_unit_errors;
mod compiler_vector;
mod json_roundtrip;
mod layout;
mod ltm_array_agg;
mod ltm_discovery_large_models;
mod ltm_dt_invariance;
// Compares xmutil-based MDL parsing against the native Rust parser, so it
// needs the optional xmutil C++ converter compiled in.
#[cfg(feature = "xmutil")]
mod mdl_equivalence;
mod mdl_roundtrip;
// metasd_macros calls load_dat/load_csv in its simulation tier, which are
// #[cfg(feature = "file_io")]-gated engine APIs; the same applies to the
// simulate* tiers, systems_roundtrip, and vdf_alias_decoder below.
#[cfg(feature = "file_io")]
mod metasd_macros;
mod roundtrip;
#[cfg(feature = "file_io")]
mod simulate;
#[cfg(feature = "file_io")]
mod simulate_ltm;
mod simulate_ltm_pinned;
#[cfg(feature = "file_io")]
mod simulate_ltm_wasm;
#[cfg(feature = "file_io")]
mod simulate_systems;
#[cfg(feature = "file_io")]
mod systems_roundtrip;
mod unit_alias_module_inference;
#[cfg(feature = "file_io")]
mod vdf_alias_decoder;
mod vdf_multidim;
mod vdf_structural_invariants;
mod wrld3_ltm_panic;
mod wrld3_unit_errors;
