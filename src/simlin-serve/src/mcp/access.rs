// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! `simlin-serve`'s [`ProjectAccess`] implementation.
//!
//! Backed by the same [`ProjectRegistry`] the HTTP/UI handlers consume, so
//! MCP-initiated edits and browser-initiated edits flow through identical
//! merge/persist primitives. Each method:
//!
//! 1. Path-traverses out of the registry root → `AccessError::NotFound`
//!    (canonicalising both ends so symlinks within the tree are accepted).
//! 2. Delegates to the same [`ProjectRegistry`] mutators the HTTP save
//!    handler uses (`get_or_init_doc`, `check_increment_and_merge`,
//!    `redirect_to_sidecar`, `refresh_after_write`) so a single `LoroDoc`
//!    backs every editor.
//! 3. Broadcasts `ProjectChanged { source: Agent }` after a successful
//!    merge so connected browser sockets remount their editors.

use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::sync::Arc;

use simlin_engine::datamodel;
use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::types::{ErrorOutput, SourceFormat};

use crate::events::{ChangeSource, WsMessage};
use crate::handlers::AppState;
use crate::hashing::content_hash;
use crate::registry::{GitState, ProjectFormat, ProjectMeta, RegistryError};
use crate::validation::{compute_baseline, validate_save_project};
use crate::writer::{SaveTarget, commit_write, resolve_save_target, serialize_project};

/// Registry-backed `ProjectAccess` impl shared by every MCP session.
///
/// Cloning is cheap (`Arc<AppState>`); rmcp's session machinery will hold
/// one of these per active connection and the underlying registry is the
/// single point of mutation.
#[derive(Clone)]
pub struct RegistryAccess {
    state: Arc<AppState>,
}

impl RegistryAccess {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

/// Map a `ProjectFormat` (the registry's on-disk-shape discriminator) onto
/// the MCP-facing `SourceFormat` enum.
///
/// `Mdl` collapses to `Xmile` because the registry's `.mdl` parser
/// returns an XMILE-shaped `datamodel::Project`; what the MCP client
/// sees is the in-memory project shape, not the on-disk extension.
/// `SdJson` maps to `NativeJson` because the registry's `.sd.json`
/// entries always hold native `json::Project` content (the SD-AI
/// schema is content-detected by simlin-mcp-core's stateless opener
/// and never reaches the registry's stored format).
fn project_format_to_source_format(format: ProjectFormat) -> SourceFormat {
    match format {
        ProjectFormat::Stmx | ProjectFormat::Xmile | ProjectFormat::Mdl => SourceFormat::Xmile,
        ProjectFormat::SdJson => SourceFormat::NativeJson,
    }
}

/// Render a relative path as a forward-slash string so the wire format
/// is platform-agnostic. Mirrors `handlers::path_to_forward_slash`; we
/// duplicate rather than re-export because the handler form is private
/// today and the broadcast envelope's wire shape is the contract that
/// matters here.
fn path_to_forward_slash(path: &Path) -> String {
    let display = path.to_string_lossy().into_owned();
    if MAIN_SEPARATOR == '/' {
        display
    } else {
        display.replace(MAIN_SEPARATOR, "/")
    }
}

/// Canonicalize `abs_path` and confirm it is a descendant of the
/// canonicalized registry root. Returns the canonicalized path on
/// success.
///
/// On any failure (path missing, escapes the root, cannot canonicalize the
/// root itself) returns `AccessError::NotFound { path }` so callers
/// uniformly surface "I cannot operate on that path" — distinguishing a
/// permission error from a genuinely missing file would leak filesystem
/// layout to MCP clients.
fn canonicalize_within_root(state: &AppState, abs_path: &Path) -> Result<PathBuf, AccessError> {
    let root_canonical = state
        .root
        .canonicalize()
        .map_err(|_| AccessError::NotFound {
            path: abs_path.to_path_buf(),
        })?;
    let canonical = abs_path.canonicalize().map_err(|_| AccessError::NotFound {
        path: abs_path.to_path_buf(),
    })?;
    if !canonical.starts_with(&root_canonical) {
        return Err(AccessError::NotFound {
            path: abs_path.to_path_buf(),
        });
    }
    Ok(canonical)
}

impl ProjectAccess for RegistryAccess {
    async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError> {
        let canonical = canonicalize_within_root(&self.state, abs_path)?;

        let doc = self
            .state
            .registry
            .get_or_init_doc(&canonical)
            .map_err(|e| -> AccessError {
                match e {
                    RegistryError::NotFound => AccessError::NotFound {
                        path: canonical.clone(),
                    },
                    RegistryError::HydrationFailed(msg) => {
                        AccessError::ParseError(anyhow::anyhow!(msg))
                    }
                    RegistryError::VersionMismatch { expected, actual } => {
                        // get_or_init_doc never reports VersionMismatch; if
                        // the registry implementation changes, surfacing it
                        // as an Access-level VersionMismatch is the closest
                        // honest mapping.
                        AccessError::VersionMismatch { expected, actual }
                    }
                }
            })?;

        let json_value = doc
            .export_canonical_json()
            .map_err(|e| AccessError::ParseError(anyhow::anyhow!("export doc: {e}")))?;
        let json_project: simlin_engine::json::Project = serde_json::from_value(json_value)
            .map_err(|e| AccessError::ParseError(anyhow::anyhow!("convert exported JSON: {e}")))?;
        let project: datamodel::Project = json_project.into();

        let meta = self
            .state
            .registry
            .get(&canonical)
            .ok_or_else(|| AccessError::NotFound {
                path: canonical.clone(),
            })?;

        Ok(OpenedProject {
            project,
            source_format: project_format_to_source_format(meta.format),
            version: meta.version,
        })
    }

    async fn save(
        &self,
        abs_path: &Path,
        project: &datamodel::Project,
        _format: SourceFormat,
        expected_version: Option<u64>,
    ) -> Result<u64, AccessError> {
        let canonical = canonicalize_within_root(&self.state, abs_path)?;

        // The MCP-supplied `format` is the project's *content shape* the
        // caller perceives (Xmile vs NativeJson vs SdaiJson). The on-disk
        // *file shape* — and therefore where the new bytes land — is
        // dictated by the registry's `ProjectFormat`, which is what's
        // currently on disk. A `.mdl` entry must always sidecar; a `.stmx`
        // entry must always overwrite in place; etc.
        let registry_meta =
            self.state
                .registry
                .get(&canonical)
                .ok_or_else(|| AccessError::NotFound {
                    path: canonical.clone(),
                })?;
        let registry_format = registry_meta.format;

        // Validate the post-edit project against a baseline of pre-edit
        // errors so saves that *fix* errors are accepted but saves that
        // *introduce* errors are rejected. The baseline is the current
        // doc state — `check_increment_and_merge` will rewrite the doc
        // shortly, so this is the last chance to capture pre-edit
        // diagnostics without a redundant disk read.
        let current_doc = self
            .state
            .registry
            .get_or_init_doc(&canonical)
            .map_err(|e| match e {
                RegistryError::NotFound => AccessError::NotFound {
                    path: canonical.clone(),
                },
                RegistryError::HydrationFailed(msg) => {
                    AccessError::ParseError(anyhow::anyhow!(msg))
                }
                RegistryError::VersionMismatch { expected, actual } => {
                    AccessError::VersionMismatch { expected, actual }
                }
            })?;
        let current_json_value = current_doc
            .export_canonical_json()
            .map_err(|e| AccessError::ParseError(anyhow::anyhow!("export current doc: {e}")))?;
        let current_json_project: simlin_engine::json::Project =
            serde_json::from_value(current_json_value).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!(
                    "convert current doc state to json::Project: {e}"
                ))
            })?;
        let current_project: datamodel::Project = current_json_project.into();
        let baseline = compute_baseline(&current_project);

        let outcome = validate_save_project(project, &baseline);
        if !outcome.new_errors.is_empty() {
            let errors: Vec<ErrorOutput> = outcome
                .new_errors
                .into_iter()
                .map(|e| ErrorOutput {
                    code: e.code,
                    message: e.message,
                    model_name: e.model_name,
                    variable_name: e.variable_name,
                    kind: e.kind,
                })
                .collect();
            return Err(AccessError::Validation { errors });
        }

        // Re-canonicalize the validated project so the merge sees the
        // engine's canonical JSON shape regardless of any subtle drift in
        // the input the MCP caller produced.
        let canonical_project: simlin_engine::json::Project = (&outcome.project).into();
        let canonical_value = serde_json::to_value(&canonical_project).map_err(|e| {
            AccessError::ParseError(anyhow::anyhow!("serialize canonical project: {e}"))
        })?;

        // When the caller doesn't pass an expected_version we just fetch
        // the current registry value: AI clients have no read-then-write
        // ergonomics, so optimistic-locking against an MCP-only conversation
        // is meaningless. Browser saves continue to pass an explicit
        // version through the HTTP handler.
        let version_check = expected_version.unwrap_or(registry_meta.version);

        let (new_version, merged_doc) = self
            .state
            .registry
            .check_increment_and_merge(&canonical, version_check, &canonical_value)
            .map_err(|e| match e {
                RegistryError::NotFound => AccessError::NotFound {
                    path: canonical.clone(),
                },
                RegistryError::VersionMismatch { expected, actual } => {
                    AccessError::VersionMismatch { expected, actual }
                }
                RegistryError::HydrationFailed(msg) => {
                    AccessError::ParseError(anyhow::anyhow!(msg))
                }
            })?;

        // Re-export the merged state so the bytes written to disk reflect
        // exactly what the doc holds (and any future server-side
        // annotations remain coherent).
        let merged_value = merged_doc
            .export_canonical_json()
            .map_err(|e| AccessError::ParseError(anyhow::anyhow!("export merged doc: {e}")))?;
        let merged_json_project: simlin_engine::json::Project =
            serde_json::from_value(merged_value).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!(
                    "convert merged doc state to json::Project: {e}"
                ))
            })?;
        let merged_project: datamodel::Project = merged_json_project.into();

        let target = resolve_save_target(&canonical, registry_format);
        let write_outcome = serialize_project(&merged_project, &target).map_err(|e| {
            AccessError::WriteError(std::io::Error::other(format!("serialize_project: {e}")))
        })?;
        let written_path = write_outcome.path.clone();
        let written_hash = content_hash(&write_outcome.bytes);

        // Prime the echo-suppression hash before the OS-visible write so
        // the file watcher's inotify event sees the new hash by the time
        // it computes one — same ordering rule the HTTP handler enforces.
        self.state
            .registry
            .prime_echo_hash(&canonical, written_hash);

        commit_write(&write_outcome).map_err(|e| {
            AccessError::WriteError(std::io::Error::other(format!("commit_write: {e}")))
        })?;

        // For SidecarJson (a `.mdl` save), the registry key moves from
        // the .mdl path to the .sd.json sidecar path. Subsequent reads
        // via either path will land on the sidecar entry.
        let registry_key: PathBuf = match &target {
            SaveTarget::SidecarJson {
                mdl_path,
                sidecar_path,
            } => {
                match self
                    .state
                    .registry
                    .redirect_to_sidecar(mdl_path, sidecar_path.clone())
                {
                    Ok(()) => sidecar_path.clone(),
                    Err(e) => {
                        // The .mdl entry was concurrently removed (e.g. by a
                        // scan between merge and redirect). Re-insert the
                        // sidecar key carrying the just-incremented version
                        // so the registry still tracks the on-disk file.
                        // Same fallback the HTTP handler uses.
                        tracing::warn!(
                            error = %e,
                            "MCP save: registry redirect_to_sidecar failed; re-inserting sidecar entry"
                        );
                        self.state.registry.upsert_max_version(
                            sidecar_path.clone(),
                            ProjectMeta {
                                path: PathBuf::new(),
                                format: ProjectFormat::SdJson,
                                mtime: std::time::SystemTime::UNIX_EPOCH,
                                size: 0,
                                git: GitState::Untracked,
                                version: new_version,
                                doc: Default::default(),
                                last_disk_hash: written_hash,
                            },
                        );
                        sidecar_path.clone()
                    }
                }
            }
            SaveTarget::InPlaceXmile(_) | SaveTarget::SdJson(_) => canonical.clone(),
        };

        // Refresh mtime/size/hash from the freshly-written file so the
        // SPA's stale-data heuristics see the new values. The hash is
        // already what `prime_echo_hash` stored; refreshing here keeps
        // the three fields atomic w.r.t. concurrent reads.
        if let Ok(metadata) = std::fs::metadata(&written_path)
            && let Ok(mtime) = metadata.modified()
        {
            self.state.registry.refresh_after_write(
                &registry_key,
                mtime,
                metadata.len(),
                written_hash,
            );
        }

        // Build the relative path used in the broadcast envelope. We use
        // the canonicalized root so symlinked-in subtrees hash to the
        // same string the HTTP handler emits.
        let root_canonical = self.state.root.canonicalize().map_err(|e| {
            AccessError::IoError(std::io::Error::other(format!(
                "canonicalize root for broadcast: {e}"
            )))
        })?;
        let rel = registry_key
            .strip_prefix(&root_canonical)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| registry_key.clone());
        let rel_str = path_to_forward_slash(&rel);

        self.state.events.publish(WsMessage::ProjectChanged {
            path: rel_str,
            version: new_version,
            source: ChangeSource::Agent,
        });

        Ok(new_version)
    }

    async fn create(
        &self,
        _abs_path: &Path,
        _project: &datamodel::Project,
        _format: SourceFormat,
    ) -> Result<(), AccessError> {
        // Implemented in Task 3.
        Err(AccessError::IoError(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "RegistryAccess::create not yet implemented",
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    use tempfile::TempDir;

    use crate::events::EventBus;
    use crate::git::GitProbe;
    use crate::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};

    const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    fn copy_fixture(name: &str, dest_dir: &Path) -> PathBuf {
        let src = PathBuf::from(FIXTURES_DIR).join(name);
        let dest = dest_dir.join(name);
        fs::copy(&src, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
        dest
    }

    fn build_state(root: PathBuf) -> Arc<AppState> {
        Arc::new(AppState {
            registry: Arc::new(ProjectRegistry::new(root.clone())),
            git: Arc::new(GitProbe::unavailable_for_tests()),
            root: Arc::new(root),
            events: Arc::new(EventBus::new()),
            launch_token: Arc::new(String::new()),
        })
    }

    fn seed_registry(state: &AppState, abs_path: &Path, format: ProjectFormat) {
        let metadata = fs::metadata(abs_path).expect("file exists");
        let mtime = metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        state.registry.upsert(
            abs_path.to_path_buf(),
            ProjectMeta {
                path: PathBuf::new(),
                format,
                mtime,
                size: metadata.len(),
                git: GitState::Untracked,
                version: 0,
                doc: Default::default(),
                last_disk_hash: 0,
            },
        );
    }

    #[tokio::test]
    async fn open_returns_project_with_format_and_version_zero() {
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let abs = copy_fixture("teacup.xmile", &canonical_root);
        let state = build_state(canonical_root.clone());
        seed_registry(&state, &abs, ProjectFormat::Xmile);

        let access = RegistryAccess::new(state);
        let opened = access.open(&abs).await.expect("open succeeds");

        assert_eq!(opened.source_format, SourceFormat::Xmile);
        assert_eq!(opened.version, 0);
        assert!(
            !opened.project.models.is_empty(),
            "teacup project should have at least one model"
        );
    }

    #[tokio::test]
    async fn open_reports_not_found_for_missing_path() {
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let state = build_state(canonical_root.clone());

        let access = RegistryAccess::new(state);
        let missing = canonical_root.join("missing.xmile");
        match access.open(&missing).await {
            Err(AccessError::NotFound { path }) => assert_eq!(path, missing),
            Err(other) => panic!("expected NotFound, got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[tokio::test]
    async fn open_rejects_paths_outside_root_as_not_found() {
        // A path that exists on disk but lives outside the registry root
        // must surface as NotFound (we deliberately do not distinguish
        // "permission denied" so we don't leak filesystem layout).
        let temp = TempDir::new().expect("tempdir");
        let outer = temp.path().canonicalize().expect("canon outer");
        let inner = outer.join("subroot");
        fs::create_dir(&inner).expect("create subroot");
        let outside = outer.join("escape.xmile");
        fs::write(&outside, b"<root/>\n").expect("write outside");

        let state = build_state(inner.clone());
        let access = RegistryAccess::new(state);

        // Use a path that traverses out of the inner root. canonicalize()
        // will resolve `inner/../escape.xmile` -> `outer/escape.xmile`,
        // which is outside `inner`. The registry must reject this even
        // though the file exists.
        let attempted = inner.join("..").join("escape.xmile");
        match access.open(&attempted).await {
            Err(AccessError::NotFound { .. }) => {}
            Err(other) => panic!("expected NotFound for path outside root, got {other:?}"),
            Ok(_) => panic!("expected NotFound for path outside root, got Ok"),
        }
    }

    #[tokio::test]
    async fn open_returns_native_json_for_sd_json_entries() {
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let abs = canonical_root.join("model.sd.json");
        // Minimal valid sd.json; the parse path is exercised end-to-end.
        let json = r#"{"name":"demo","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        fs::write(&abs, json).expect("write sd.json");
        let state = build_state(canonical_root.clone());
        seed_registry(&state, &abs, ProjectFormat::SdJson);

        let access = RegistryAccess::new(state);
        let opened = access.open(&abs).await.expect("open succeeds");
        assert_eq!(opened.source_format, SourceFormat::NativeJson);
        assert_eq!(opened.version, 0);
    }

    #[tokio::test]
    async fn open_carries_through_registry_version_after_increment() {
        // Demonstrates the registry-shared-state property: when another
        // path bumps the entry's version (Phase 3's
        // `check_increment_and_merge`), the next MCP open sees the new
        // value.
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let abs = canonical_root.join("model.sd.json");
        let json = r#"{"name":"demo","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        fs::write(&abs, json).expect("write sd.json");
        let state = build_state(canonical_root.clone());
        seed_registry(&state, &abs, ProjectFormat::SdJson);

        // Drive the version forward via the same primitive a browser save uses.
        let updated = serde_json::json!({
            "name":"renamed",
            "simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},
            "models":[{"name":"main"}]
        });
        let (v, _doc) = state
            .registry
            .check_increment_and_merge(&abs, 0, &updated)
            .expect("merge");
        assert_eq!(v, 1);

        let access = RegistryAccess::new(state.clone());
        let opened = access.open(&abs).await.expect("open after merge");
        assert_eq!(opened.version, 1);
        assert_eq!(opened.project.name, "renamed");
    }
}
