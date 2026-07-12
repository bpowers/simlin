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
//! Feature gating: `file_io` is always on for this harness -- the crate takes a
//! self dev-dependency that enables it (see Cargo.toml), because `file_io`
//! gates a production dependency, not test coverage (GH #925). `xmutil` gates a
//! heavyweight optional C++ converter that only one module compares against, so
//! that one module stays `#[cfg]`-gated below. A `cfg`-gated module compiles to
//! nothing when its feature is off, which has the same effect as the
//! `required-features` entries these replaced (the tests only exist when the
//! feature is enabled) without skipping the whole harness.

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
mod metasd_macros;
mod roundtrip;
mod simulate;
mod simulate_ltm;
mod simulate_ltm_pinned;
mod simulate_ltm_wasm;
mod simulate_systems;
mod systems_roundtrip;
mod unit_alias_module_inference;
mod vdf_alias_decoder;
mod vdf_data_grid;
mod vdf_multidim;
// Differential parity harness: pins the Rust reader against the Python
// tools/vdf_xray.py inspector by shelling out to python3 (no cargo feature
// needed; python3 is already a repo prerequisite via the pysimlin pre-commit
// step).
mod vdf_parity;
mod vdf_sensitivity;
mod vdf_structural_invariants;
mod wrld3_ltm_panic;
mod wrld3_unit_errors;
