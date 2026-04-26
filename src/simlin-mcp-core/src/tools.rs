// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! Async tool implementations exposed to MCP transports.
//!
//! Each tool is an `async fn` parameterised over `A: ProjectAccess` so
//! the same code path serves both the stdio binary (with a stateless
//! filesystem-backed access impl) and the Phase 6 HTTP host (with a
//! `ProjectRegistry`-backed impl).  The functions are deliberately
//! transport-agnostic — they take a typed input, return a typed output,
//! and surface failures via `AccessError`.  rmcp glue (Task 6) wraps
//! these in `CallToolResult`.

pub mod create_model;
pub mod read_model;
