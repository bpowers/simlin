// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
//! Error types raised by [`crate::access::ProjectAccess`] and its callers.
//!
//! `AccessError` is the trait-level error: it covers everything that can
//! go wrong while loading or persisting a project.  `Validation` carries
//! engine-level diagnostics as [`ErrorOutput`] entries — the same wire
//! shape `simlin-mcp` already exposes — so the rmcp tool layer can
//! serialise validation failures without a translation step.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::types::ErrorOutput;

/// Failure modes for [`crate::access::ProjectAccess`].
#[derive(Debug)]
pub enum AccessError {
    /// The project file at `path` does not exist (or, for a registry-backed
    /// implementation, has no entry under `path`).
    NotFound { path: PathBuf },
    /// An I/O error occurred while reading the project bytes.
    IoError(io::Error),
    /// The project bytes failed to parse.  We use `anyhow::Error` because
    /// the underlying engine error types vary by format (XMILE / MDL /
    /// JSON) and the MCP layer only needs a human-readable message.
    ParseError(anyhow::Error),
    /// `save` was called with `expected_version` that does not match the
    /// current registry version (registry-backed impls only).
    VersionMismatch { expected: u64, actual: u64 },
    /// An I/O error occurred while writing the project bytes.
    WriteError(io::Error),
    /// The post-edit project failed engine-level diagnostics.  These are
    /// surfaced verbatim to clients so an LLM can reason about what
    /// went wrong.  Carrying [`ErrorOutput`] keeps the wire shape
    /// identical to ReadModel/EditModel's `errors` field.
    Validation { errors: Vec<ErrorOutput> },
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccessError::NotFound { path } => write!(f, "project not found: {}", path.display()),
            AccessError::IoError(err) => write!(f, "i/o error: {err}"),
            AccessError::ParseError(err) => write!(f, "parse error: {err}"),
            AccessError::VersionMismatch { expected, actual } => write!(
                f,
                "project version mismatch: expected {expected}, actual {actual}"
            ),
            AccessError::WriteError(err) => write!(f, "write error: {err}"),
            AccessError::Validation { errors } => {
                write!(f, "validation failed ({} error(s))", errors.len())
            }
        }
    }
}

impl Error for AccessError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            AccessError::IoError(err) | AccessError::WriteError(err) => Some(err),
            AccessError::ParseError(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}
