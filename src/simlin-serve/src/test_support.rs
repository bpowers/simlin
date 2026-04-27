// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Test-only helpers exposed for integration tests under `tests/`.
//!
//! Mirrors the `test_support` pattern from `simlin-mcp-core`: a
//! `#[doc(hidden)]` module so integration tests can import helpers without
//! polluting the public library API.

use crate::git::GitProbe;

/// Return a `GitProbe` that behaves as if git is unavailable.
///
/// Use this in integration tests to exercise the AC2.5 degraded-state path
/// (every file reports `GitState::Unavailable`) without requiring the host
/// to have git installed or a working repository at hand.
pub fn unavailable_git_probe() -> GitProbe {
    GitProbe::new_unavailable()
}
