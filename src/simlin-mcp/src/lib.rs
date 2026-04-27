// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Library half of the `simlin-mcp` crate.
//!
//! The binary entry point lives in `main.rs` and is intentionally thin —
//! it composes resources, builds a `SimlinMcpServer<FileSystemAccess>`,
//! and hands it to rmcp's stdio transport.  Everything reusable (the
//! `FileSystemAccess` impl in particular) lives here so integration
//! tests can exercise it directly without spawning the binary.

pub mod access;
