// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! Async tool implementations exposed to MCP transports.
//!
//! Each tool is an `async fn` parameterised over `A: ProjectAccess`, so it
//! does not know which backing store it reads and writes.  The functions
//! are deliberately transport-agnostic — they take a typed input, return a
//! typed output, and surface failures via `AccessError`.  The rmcp glue in
//! [`crate::server`] wraps these in `CallToolResult`.

pub mod create_model;
pub mod edit_model;
pub mod read_model;
