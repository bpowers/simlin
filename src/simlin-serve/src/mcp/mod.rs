// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! In-process MCP integration mounted alongside the HTTP/UI router.
//!
//! Phase 6 builds a `simlin-mcp-core`-backed MCP server inside `simlin-serve`
//! that shares the same `ProjectRegistry` (and therefore the same in-memory
//! `LoroDoc` state, version counter, and event bus) as the browser-facing
//! handlers. The submodules layer in over the next phases:
//!
//! - `access` — `RegistryAccess` impl of `simlin_mcp_core::ProjectAccess`.
//! - (later) `server` — the rmcp `ServerHandler` that mounts the tool surface.
//! - (later) `transport` — the streamable-HTTP service factory.

pub mod access;

pub use access::RegistryAccess;
