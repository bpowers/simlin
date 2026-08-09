// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Library half of the `simlin-mcp` crate.
//!
//! The binary entry point lives in `main.rs` and is intentionally thin —
//! it composes resources, builds a `SimlinMcpServer<FileSystemAccess>`,
//! and hands it to rmcp's stdio transport.
//!
//! `FileSystemAccess` itself lives in `simlin-mcp-core` and is re-exported
//! here under its historical path. It moved so that `simlin-mcp-core`'s own
//! integration suites run against the shipping impl rather than a
//! hand-maintained copy of it (see `simlin_mcp_core::fs_access`); the
//! re-export keeps every existing `simlin_mcp::access::FileSystemAccess`
//! import working.

pub mod access {
    pub use simlin_mcp_core::fs_access::FileSystemAccess;
}
