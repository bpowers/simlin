// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Stateless filesystem-backed [`ProjectAccess`] for the `simlin-mcp` binary.
//!
//! Each tool call re-reads the file from disk and writes the result back
//! verbatim — this preserves the wire-level semantics of the pre-rmcp
//! `@simlin/mcp` server, where there is no in-memory project cache and
//! every call sees the file's current bytes.
//!
//! `expected_version` is ignored on `save` because there is no shared
//! state to lock against; we always return `0` (the same constant
//! [`ProjectAccess::open`] supplies).  When `simlin-serve` mounts the
//! same handler in Phase 6 it provides its own `ProjectAccess` impl that
//! actually honours the version token.
//!
//! `.mdl` files are write-rejected here so an LLM gets a single,
//! actionable error message rather than the engine's deeper "MDL writer
//! is not implemented" failure.  The exact string is matched verbatim by
//! existing `@simlin/mcp` clients, so it must not change.

use std::io;
use std::path::Path;

use simlin_engine::datamodel;
use simlin_engine::json as ejson;
use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::open::open_project;
use simlin_mcp_core::types::SourceFormat;

/// Stateless filesystem-backed `ProjectAccess`.
///
/// Holds no state — construction is free, cloning is free, and there are
/// no concurrency guarantees beyond what the operating system provides
/// for individual `read`/`write` syscalls.
#[derive(Debug, Default, Clone, Copy)]
pub struct FileSystemAccess;

impl FileSystemAccess {
    pub const fn new() -> Self {
        Self
    }
}

impl ProjectAccess for FileSystemAccess {
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
        // .mdl write rejection: the canonical message must match exactly
        // because @simlin/mcp clients render it verbatim.  We check the
        // path's extension rather than the format because `.mdl` files
        // are parsed as Xmile internally — only the on-disk extension
        // distinguishes a Vensim-source project from an XMILE one here.
        if has_mdl_extension(abs_path) {
            return Err(AccessError::WriteError(io::Error::new(
                io::ErrorKind::Unsupported,
                "Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, \
                 then CreateModel to start a new .sd.json file you can edit.",
            )));
        }

        let bytes = serialize_project(project, format)?;
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(0)
    }

    async fn create(
        &self,
        abs_path: &Path,
        project: &datamodel::Project,
        format: SourceFormat,
    ) -> Result<(), AccessError> {
        // tokio::fs::try_exists distinguishes "file is missing" from
        // "permission denied", which a plain metadata() call cannot.
        let exists = tokio::fs::try_exists(abs_path)
            .await
            .map_err(AccessError::WriteError)?;
        if exists {
            return Err(AccessError::WriteError(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("file already exists: {}", abs_path.display()),
            )));
        }

        if let Some(parent) = abs_path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(AccessError::WriteError)?;
        }

        let bytes = serialize_project(project, format)?;
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(())
    }
}

fn has_mdl_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mdl"))
}

/// Serialise `project` to bytes in the requested `format`.
///
/// SdaiJson outputs include a derived `relationships` field computed
/// from the engine's salsa-backed link-polarity analysis — this matches
/// the pre-rmcp simlin-mcp behaviour where every save re-derived
/// relationships rather than trusting whatever was on disk.  The
/// in-test path used by `simlin-mcp-core/tests/integration/edit_model_e2e.rs` skips
/// this enrichment because it doesn't exercise SD-AI relationships;
/// production callers go through this function and must keep
/// relationships in sync with the post-edit model.
fn serialize_project(
    project: &datamodel::Project,
    format: SourceFormat,
) -> Result<Vec<u8>, AccessError> {
    match format {
        SourceFormat::Xmile => {
            let xml = simlin_engine::to_xmile(project).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to serialize XMILE: {e:?}"))
            })?;
            Ok(xml.into_bytes())
        }
        SourceFormat::NativeJson => {
            let json_project = ejson::Project::from(project);
            serde_json::to_vec_pretty(&json_project).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to serialize JSON: {e}"))
            })
        }
        SourceFormat::SdaiJson => {
            let mut sdai_model = simlin_engine::json_sdai::SdaiModel::from(project);
            // Preserve the existing simlin-mcp semantic: relationships are
            // generated from the post-edit model's equation-dependency
            // polarities, not preserved from whatever was in the source
            // file.  Errors here are non-fatal — a missing model just
            // means relationships stays None, which the SD-AI conformance
            // evaluator expects to be regenerated independently.
            if let Some(model_name) = project.models.first().map(|m| m.name.clone()) {
                let db = simlin_engine::db::SimlinDb::default();
                let sync = simlin_engine::db::sync_from_datamodel(&db, project);
                let canonical_name = simlin_engine::canonicalize(&model_name).into_owned();
                if let Some(source_model) = sync.project.models(&db).get(&canonical_name).copied()
                    && let Some(dm_model) = project.get_model(&model_name)
                {
                    let polarities =
                        simlin_engine::db::compute_link_polarities(&db, source_model, sync.project);
                    sdai_model.relationships = Some(
                        simlin_engine::json_sdai::generate_relationships(&polarities, dm_model),
                    );
                }
            }
            serde_json::to_vec_pretty(&sdai_model).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to serialize SD-AI JSON: {e}"))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_mdl_extension_is_case_insensitive() {
        assert!(has_mdl_extension(Path::new("foo.mdl")));
        assert!(has_mdl_extension(Path::new("foo.MDL")));
        assert!(has_mdl_extension(Path::new("FOO.Mdl")));
        assert!(!has_mdl_extension(Path::new("foo.sd.json")));
        assert!(!has_mdl_extension(Path::new("foo.stmx")));
        assert!(!has_mdl_extension(Path::new("foo")));
    }
}
