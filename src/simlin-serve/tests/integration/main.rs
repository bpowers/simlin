// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Single integration-test harness for simlin-serve.
//!
//! All integration tests are modules of this one binary rather than separate
//! `tests/*.rs` files. Each top-level test file becomes its own ~80MB binary
//! that statically links the full dependency graph, and on macOS every fresh
//! binary pays a serialized first-exec security scan (~1-3s each), which blew
//! the pre-commit `cargo test` wall-clock budget. One harness per crate keeps
//! link time, disk, and scan cost constant as tests grow. See GH issue #706.
//!
//! Add new integration tests as a `mod` here, not as a new file directly
//! under `tests/`. Shared fixtures stay in `tests/fixtures/` (referenced via
//! `CARGO_MANIFEST_DIR`, so the extra directory level doesn't affect them).
//!
//! Note: tests that formerly lived in separate binaries now run as threads of
//! one process, so the libtest scheduler interleaves them. That's safe here
//! because no test mutates process-global state (no `set_var`, no
//! `set_current_dir`, no fixed-port binds: in-process router tests use
//! `tower::oneshot`, and the bind-time tests use OS-assigned port 0).

#![deny(unsafe_code)]

mod api_get_project;
mod api_projects;
mod api_save;
mod build_npm_packages;
mod diagnostics_events;
mod discovery_integration;
mod dual_port_smoke;
mod e2e_live_update;
mod e2e_mcp_browser;
mod e2e_smoke;
mod git_integration;
mod healthz;
mod mcp_registry_access;
mod mcp_tool_surface;
mod middleware_host;
mod parity_create;
mod router_layers;
mod serve_release_workflow;
// Excluded on Windows due to a Windows-only atomic_write/watcher race in the
// spawned binary; see the module-level docs in smoke.rs (tech-debt item #38).
// The cfg used to be an inner `#![cfg]` in the file itself.
#[cfg(not(target_os = "windows"))]
mod smoke;
mod static_assets;
mod watcher_git;
mod watcher_merge;
mod watcher_smoke;
mod ws_updates;
