// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
//! Re-exports of MCP tool output types from `simlin-mcp-core`.
//!
//! The canonical definitions live in `simlin_mcp_core::types`.  Existing
//! tool wrappers in this binary import from here as `super::types::...`,
//! so this shim keeps the call sites compiling while the canonical
//! versions are owned by the library.

pub use simlin_mcp_core::types::{DominantPeriodOutput, ErrorOutput, LoopDominanceSummary};
