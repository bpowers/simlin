// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Transport-agnostic core for the Simlin MCP server.
//!
//! `simlin-mcp-core` owns the MCP tool implementations, format detection
//! helpers, output types, and the rmcp `ServerHandler` impl.  The
//! `simlin-mcp` stdio binary mounts the tool surface by choosing a
//! transport and a [`ProjectAccess`] implementation.

#![deny(unsafe_code)]

pub mod access;
pub mod errors;
pub mod fs_access;
pub mod open;
mod relationships;
pub mod server;
/// Fixtures for integration tests. Gated behind the `test-support` feature so
/// it is not compiled into shipped binaries; the crate's own integration
/// tests reach it through a self dev-dependency that enables the feature.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;
pub mod tools;
pub mod types;

pub use access::{OpenedProject, ProjectAccess, SaveOutcome};
pub use errors::AccessError;
pub use fs_access::FileSystemAccess;
pub use server::{ResourceContent, SimlinMcpServer};
pub use types::{
    DominantPeriodOutput, ErrorOutput, LoopDominanceSummary, PartitionOutput, SourceFormat,
};
