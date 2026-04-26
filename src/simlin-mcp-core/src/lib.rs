// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Transport-agnostic core for the Simlin MCP server.
//!
//! `simlin-mcp-core` owns the shared MCP tool implementations, format
//! detection helpers, output types, and (in later tasks) the rmcp
//! `ServerHandler` impl.  Both the `simlin-mcp` stdio binary and the
//! HTTP-mounted `simlin-serve` mount the same tool surface against this
//! crate by providing their own [`ProjectAccess`] implementation.

#![deny(unsafe_code)]

pub mod access;
pub mod errors;
pub mod open;
pub mod tools;
pub mod types;

pub use access::{OpenedProject, ProjectAccess};
pub use errors::AccessError;
pub use types::{DominantPeriodOutput, ErrorOutput, LoopDominanceSummary, SourceFormat};
