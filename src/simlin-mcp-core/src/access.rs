// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
//! Abstraction for opening and persisting Simlin projects.
//!
//! Tools in this crate operate against the [`ProjectAccess`] trait so the
//! same async tool functions can run unchanged against either a stateless
//! filesystem-backed implementation (the `simlin-mcp` stdio binary) or a
//! `ProjectRegistry`-backed implementation (the `simlin-serve` HTTP host
//! introduced in Phase 6).
//!
//! The trait deliberately uses native async-fn-in-trait (AFIT) rather than
//! the `async-trait` crate.  rmcp's macro-generated dispatch wants concrete
//! handler types, so callers always know `A` statically; we never need
//! `dyn ProjectAccess` and therefore do not pay for `async-trait`'s heap
//! allocation.

use std::path::Path;

use crate::errors::AccessError;
use crate::types::SourceFormat;

/// A snapshot of a project loaded from some backing store, together with
/// the metadata needed to write it back consistently.
///
/// `version` is an optional concurrency token: stateless implementations
/// always return `0`; registry-backed implementations return the
/// `ProjectRegistry`'s monotonically increasing version so callers can
/// pass it back to `save` for optimistic-locking.
pub struct OpenedProject {
    pub project: simlin_engine::datamodel::Project,
    pub source_format: SourceFormat,
    pub version: u64,
}

/// Loads, persists, and creates Simlin projects from some backing store.
///
/// All methods take `abs_path` as the canonical identifier of the
/// project; backends are free to interpret this either as a filesystem
/// path (stateless impl) or as a registry key (server impl) provided
/// they accept the absolute paths produced by callers.
///
/// `expected_version` on [`save`] is the optimistic-locking token: pass
/// `None` to skip the check (stateless impl), or pass the value returned
/// by a previous [`open`]/[`save`] to detect concurrent writers.  Both
/// impls return the new post-write version.
pub trait ProjectAccess: Send + Sync + 'static {
    fn open(
        &self,
        abs_path: &Path,
    ) -> impl Future<Output = Result<OpenedProject, AccessError>> + Send;

    fn save(
        &self,
        abs_path: &Path,
        project: &simlin_engine::datamodel::Project,
        format: SourceFormat,
        expected_version: Option<u64>,
    ) -> impl Future<Output = Result<u64, AccessError>> + Send;

    fn create(
        &self,
        abs_path: &Path,
        project: &simlin_engine::datamodel::Project,
        format: SourceFormat,
    ) -> impl Future<Output = Result<(), AccessError>> + Send;
}
