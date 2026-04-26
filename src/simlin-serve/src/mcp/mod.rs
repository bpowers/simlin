// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! In-process MCP integration mounted alongside the HTTP/UI router.
//!
//! Phase 6 builds a `simlin-mcp-core`-backed MCP server inside `simlin-serve`
//! that shares the same `ProjectRegistry` (and therefore the same in-memory
//! `LoroDoc` state, version counter, and event bus) as the browser-facing
//! handlers. The submodules layer in over the next phases:
//!
//! - `access` — `RegistryAccess` impl of `simlin_mcp_core::ProjectAccess`.
//! - `server` — the rmcp `ServerHandler` that mounts the tool surface.
//! - `transport` — the streamable-HTTP service factory mounted at `/mcp`.

pub mod access;
pub mod list_projects;
pub mod server;
pub mod simulate;
pub mod transport;

pub use access::RegistryAccess;
pub use server::SimlinServeMcpServer;
pub use transport::build_mcp_router;
