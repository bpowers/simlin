// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Shared filesystem-backed [`ProjectAccess`] implementation for integration
//! tests.
//!
//! Each of the per-tool integration test suites previously defined its own
//! local `FsAccess` struct with the same logic. This module consolidates
//! them so changes to the serialisation path are made once and all test
//! suites pick them up.
//!
//! Exposed as `#[doc(hidden)]` so that integration tests under `tests/` can
//! import it (`use simlin_mcp_core::test_support::TestFileSystemAccess`)
//! without polluting the public library API.

use std::io;
use std::path::Path;

use simlin_engine::datamodel;
use simlin_engine::json as ejson;

use crate::access::{OpenedProject, ProjectAccess};
use crate::errors::AccessError;
use crate::open::open_project;
use crate::types::SourceFormat;

/// Stateless filesystem-backed `ProjectAccess` for integration tests.
///
/// Mirrors the production `FileSystemAccess` in the `simlin-mcp` binary
/// without depending on that crate so tests in `simlin-mcp-core` remain
/// self-contained.  Serialisation and atomic-write semantics are
/// intentionally identical to the production impl.
pub struct TestFileSystemAccess;

impl ProjectAccess for TestFileSystemAccess {
    async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError> {
        let contents = tokio::fs::read_to_string(abs_path).await.map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                AccessError::NotFound {
                    path: abs_path.to_path_buf(),
                }
            } else {
                AccessError::IoError(e)
            }
        })?;
        let (project, source_format) = open_project(abs_path, &contents)?;
        Ok(OpenedProject {
            project,
            source_format,
            version: 0,
        })
    }

    async fn save(
        &self,
        abs_path: &Path,
        project: &datamodel::Project,
        format: SourceFormat,
        _expected_version: Option<u64>,
    ) -> Result<u64, AccessError> {
        let bytes = serialize(project, format)?;
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(0)
    }

    async fn create(
        &self,
        abs_path: &Path,
        project: &datamodel::Project,
        format: SourceFormat,
    ) -> Result<(), AccessError> {
        if abs_path.exists() {
            return Err(AccessError::WriteError(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("file already exists: {}", abs_path.display()),
            )));
        }
        if let Some(parent) = abs_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(AccessError::WriteError)?;
        }
        let bytes = serialize(project, format)?;
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(())
    }
}

fn serialize(project: &datamodel::Project, format: SourceFormat) -> Result<Vec<u8>, AccessError> {
    match format {
        SourceFormat::Xmile => simlin_engine::to_xmile(project)
            .map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to serialize XMILE: {e:?}"))
            })
            .map(String::into_bytes),
        SourceFormat::NativeJson => {
            let json_project = ejson::Project::from(project);
            serde_json::to_vec_pretty(&json_project)
                .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize: {e}")))
        }
        SourceFormat::SdaiJson => {
            let sdai_model = simlin_engine::json_sdai::SdaiModel::from(project);
            serde_json::to_vec_pretty(&sdai_model)
                .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize: {e}")))
        }
    }
}
