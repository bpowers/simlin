// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Stateless filesystem-backed [`ProjectAccess`], used by the `simlin-mcp`
//! stdio binary and by this crate's own integration tests.
//!
//! Each tool call re-reads the file from disk and writes the result back
//! verbatim — this preserves the wire-level semantics of the pre-rmcp
//! `@simlin/mcp` server, where there is no in-memory project cache and
//! every call sees the file's current bytes.
//!
//! It lives here rather than in the binary so the tests and the binary run
//! the SAME impl: `test_support::TestFileSystemAccess` is a type alias for
//! this struct, never a second implementation. A hand-maintained near-copy
//! drifts at exactly the points where this file is non-trivial (the MDL
//! lossiness-warning channel and the SD-AI `relationships` regeneration on
//! save), so a test saving through a copy proves something about a simpler
//! function than the one that ships.
//!
//! `expected_version` is ignored on `save` because there is no shared
//! state to lock against; we always return `0` (the same constant
//! [`ProjectAccess::open`] supplies).  `simlin-serve` provides its own
//! registry-backed `ProjectAccess` impl that actually honours the token.
//!
//! `.mdl` files are written back in place as regenerated Vensim text (the
//! engine's `to_mdl_with_warnings`), the same way `.stmx` is regenerated
//! XMILE.  Constructs Vensim cannot express are written in their closest
//! representable form and reported through [`SaveOutcome::warnings`]; they
//! never fail the save.

use std::io;
use std::path::Path;

use simlin_engine::datamodel;
use simlin_engine::json as ejson;

use crate::access::{OpenedProject, ProjectAccess, SaveOutcome};
use crate::errors::AccessError;
use crate::open::open_project;
use crate::types::{ErrorOutput, SourceFormat, mdl_export_warnings_to_outputs};

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
    ) -> Result<SaveOutcome, AccessError> {
        let (bytes, warnings) = serialize_project(project, format)?;
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(SaveOutcome {
            version: 0,
            warnings,
        })
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

        // `create` has no warnings channel: the only production caller
        // (`tools::create_model`) writes an empty project, which no writer
        // degrades. A caller that reaches `create` directly with a
        // populated project would lose MDL lossiness warnings here; use
        // `save` for that.
        let (bytes, _warnings) = serialize_project(project, format)?;
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(())
    }
}

/// Serialise `project` to bytes in the requested `format`, plus any
/// non-fatal lossiness warnings the writer raised (only the MDL writer has
/// any today; every other arm returns an empty list).
///
/// SdaiJson outputs include a derived `relationships` field computed
/// from the engine's salsa-backed link-polarity analysis — this matches
/// the pre-rmcp simlin-mcp behaviour where every save re-derived
/// relationships rather than trusting whatever was on disk.  Preserving
/// stale relationships from the source file would leave the SD-AI
/// conformance evaluator reading a causal graph that no longer matches
/// the model's equations.
fn serialize_project(
    project: &datamodel::Project,
    format: SourceFormat,
) -> Result<(Vec<u8>, Vec<ErrorOutput>), AccessError> {
    match format {
        SourceFormat::Xmile => {
            let xml = simlin_engine::to_xmile(project).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to serialize XMILE: {e:?}"))
            })?;
            Ok((xml.into_bytes(), Vec::new()))
        }
        SourceFormat::Mdl => {
            // Hard errors here are the writer's structural impossibilities
            // (more than one non-macro model, an ordinary Module variable);
            // they fail the save because the alternative is corrupt Vensim
            // text.  Degraded-but-representable constructs come back as
            // warnings and the write proceeds.
            let (text, warnings) = simlin_engine::to_mdl_with_warnings(project).map_err(|e| {
                AccessError::WriteError(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("failed to serialize Vensim MDL: {e}"),
                ))
            })?;
            Ok((
                text.into_bytes(),
                mdl_export_warnings_to_outputs(project, &warnings),
            ))
        }
        SourceFormat::NativeJson => {
            let json_project = ejson::Project::from(project);
            let bytes = serde_json::to_vec_pretty(&json_project).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to serialize JSON: {e}"))
            })?;
            Ok((bytes, Vec::new()))
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
            let bytes = serde_json::to_vec_pretty(&sdai_model).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to serialize SD-AI JSON: {e}"))
            })?;
            Ok((bytes, Vec::new()))
        }
    }
}
