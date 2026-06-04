// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Single integration-test harness for simlin-mcp-core.
//!
//! All integration tests are modules of this one binary rather than separate
//! `tests/*.rs` files. Each top-level test file becomes its own ~80MB binary
//! that statically links the full dependency graph, and on macOS every fresh
//! binary pays a serialized first-exec security scan (~1-3s each), which blew
//! the pre-commit `cargo test` wall-clock budget. One harness per crate keeps
//! link time, disk, and scan cost constant as tests grow. See GH issue #706.
//!
//! Add new integration tests as a `mod` here, not as a new file directly
//! under `tests/`.

mod create_model_e2e;
mod edit_model_e2e;
mod read_model_e2e;
mod server;
mod tool_dispatch;
