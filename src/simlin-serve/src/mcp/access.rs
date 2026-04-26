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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use simlin_engine::datamodel;
use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::types::{ErrorOutput, SourceFormat};

use crate::events::{ChangeSource, WsMessage};
use crate::handlers::AppState;
use crate::hashing::content_hash;
use crate::path_resolution::{is_mdl_extension, sidecar_for_mdl, to_forward_slash};
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

/// Wrap [`crate::path_resolution::resolve_create_target`] to surface
/// failures as `AccessError`. We canonicalize the registry root once
/// and route every `OutOfRoot` rejection (including symlink escapes
/// and `..` traversals) to `NotFound` so MCP clients cannot
/// distinguish "exists but forbidden" from "missing" — same posture as
/// [`canonicalize_within_root`].
fn resolve_create_path_within_root(
    state: &AppState,
    abs_path: &Path,
) -> Result<PathBuf, AccessError> {
    let root_canonical = state
        .root
        .canonicalize()
        .map_err(|_| AccessError::NotFound {
            path: abs_path.to_path_buf(),
        })?;
    crate::path_resolution::resolve_create_target(abs_path, &root_canonical).map_err(
        |err| match err {
            crate::path_resolution::CreatePathError::OutOfRoot => AccessError::NotFound {
                path: abs_path.to_path_buf(),
            },
            crate::path_resolution::CreatePathError::IoError(_) => AccessError::NotFound {
                path: abs_path.to_path_buf(),
            },
        },
    )
}

/// Pick the on-disk `ProjectFormat` for a fresh file based on its
/// extension. This is the writer-side analogue of the reader-side
/// `format_for_path` in `handlers.rs`. We dispatch on extension rather
/// than the caller-supplied `SourceFormat` because the caller's
/// perception of the project's *content* shape (Xmile vs NativeJson)
/// can disagree with how the file is stored on disk; the on-disk
/// extension is authoritative for the registry entry.
///
/// `.mdl` is rejected: simlin-mcp's read-only-mdl semantics extend to
/// `create` — agents that want to author a new model produce
/// `.stmx`/`.xmile`/`.sd.json` files. Subsequent saves can sidecar a
/// pre-existing `.mdl`, but agents do not introduce new ones.
fn project_format_for_create(abs_path: &Path) -> Result<ProjectFormat, AccessError> {
    let path_str = abs_path.to_string_lossy().to_lowercase();
    if path_str.ends_with(".sd.json") {
        return Ok(ProjectFormat::SdJson);
    }
    let ext = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("stmx") => Ok(ProjectFormat::Stmx),
        Some("xmile") | Some("xml") => Ok(ProjectFormat::Xmile),
        Some("mdl") => Err(AccessError::WriteError(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            ".mdl files are read-only. Use .stmx, .xmile, or .sd.json for new models.",
        ))),
        _ => Err(AccessError::WriteError(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "unrecognized file extension for create: {}",
                abs_path.display()
            ),
        ))),
    }
}

/// Serialize `project` to bytes appropriate for the on-disk `ProjectFormat`.
/// XMILE files use the engine's `to_xmile` (the same byte-stable
/// serializer the writer module uses for in-place saves); JSON files
/// use pretty-printed JSON for git-friendly diffs.
fn serialize_for_create(
    project: &datamodel::Project,
    format: ProjectFormat,
) -> Result<Vec<u8>, AccessError> {
    match format {
        ProjectFormat::Stmx | ProjectFormat::Xmile => {
            let xmile = simlin_engine::to_xmile(project)
                .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize XMILE: {e:?}")))?;
            Ok(xmile.into_bytes())
        }
        ProjectFormat::SdJson => {
            let json_project = simlin_engine::json::Project::from(project);
            serde_json::to_vec_pretty(&json_project)
                .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize JSON: {e}")))
        }
        ProjectFormat::Mdl => Err(AccessError::WriteError(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            ".mdl write is not supported by RegistryAccess::create",
        ))),
    }
}

impl ProjectAccess for RegistryAccess {
    async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError> {
        let canonical = canonicalize_within_root(&self.state, abs_path)?;

        // Sidecar preference for `.mdl` reads: when a sibling `.sd.json`
        // exists, route the open to the sidecar so MCP and HTTP return
        // the same content for the same path. Without this, an MCP
        // `ReadModel` of `foo.mdl` after a save returns either NotFound
        // (when redirect_to_sidecar removed the .mdl entry) or stale
        // .mdl bytes (when a scan re-inserted the entry); HTTP
        // `get_project` always follows the sidecar via `sidecar.is_file()`.
        // The disk check is the single source of truth — even if a stale
        // .mdl-keyed entry lingers in the registry, the sidecar wins.
        let resolved = if is_mdl_extension(&canonical) {
            let sidecar = sidecar_for_mdl(&canonical);
            if sidecar.is_file() {
                // Canonicalize the sidecar path as well; symlinks within
                // the watched tree must resolve to the same key the
                // save handler / scanner / watcher would use.
                match sidecar.canonicalize() {
                    Ok(p) => p,
                    Err(_) => canonical.clone(),
                }
            } else {
                canonical.clone()
            }
        } else {
            canonical.clone()
        };

        let doc = self
            .state
            .registry
            .get_or_init_doc(&resolved)
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
                    RegistryError::AlreadyExists => AccessError::ParseError(anyhow::anyhow!(
                        "unexpected AlreadyExists from get_or_init_doc"
                    )),
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
            .get(&resolved)
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

        // Sidecar-preference: when the caller passes a `.mdl` path that
        // already has a sibling `.sd.json` on disk, route the save to the
        // sidecar key. Mirrors `open`'s preference rule and the
        // HTTP save handler, so MCP `EditModel(.mdl)` after a prior save
        // surfaces a real version-mismatch (not a NotFound or a stale
        // overwrite) instead of bypassing optimistic locking.
        let resolved = if is_mdl_extension(&canonical) {
            let sidecar = sidecar_for_mdl(&canonical);
            if sidecar.is_file() {
                match sidecar.canonicalize() {
                    Ok(p) => p,
                    Err(_) => canonical.clone(),
                }
            } else {
                canonical.clone()
            }
        } else {
            canonical.clone()
        };

        // The MCP-supplied `format` is the project's *content shape* the
        // caller perceives (Xmile vs NativeJson vs SdaiJson). The on-disk
        // *file shape* — and therefore where the new bytes land — is
        // dictated by the registry's `ProjectFormat`, which is what's
        // currently on disk. A `.mdl` entry must always sidecar; a `.stmx`
        // entry must always overwrite in place; etc.
        let registry_meta =
            self.state
                .registry
                .get(&resolved)
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
            .get_or_init_doc(&resolved)
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
                RegistryError::AlreadyExists => AccessError::ParseError(anyhow::anyhow!(
                    "unexpected AlreadyExists from get_or_init_doc"
                )),
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
            .check_increment_and_merge(&resolved, version_check, &canonical_value)
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
                RegistryError::AlreadyExists => AccessError::ParseError(anyhow::anyhow!(
                    "unexpected AlreadyExists from check_increment_and_merge"
                )),
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

        let target = resolve_save_target(&resolved, registry_format);
        let write_outcome = serialize_project(&merged_project, &target).map_err(|e| {
            AccessError::WriteError(std::io::Error::other(format!("serialize_project: {e}")))
        })?;
        let written_path = write_outcome.path.clone();
        let written_hash = content_hash(&write_outcome.bytes);

        // Prime the echo-suppression hash before the OS-visible write so
        // the file watcher's inotify event sees the new hash by the time
        // it computes one — same ordering rule the HTTP handler enforces.
        self.state.registry.prime_echo_hash(&resolved, written_hash);

        // Sidecar saves: the watcher event fires for .sd.json, not the
        // .mdl key. Pre-establish a sidecar placeholder so the watcher
        // echo-suppresses against the primed hash even if it processes
        // its event before redirect_to_sidecar runs. See
        // ProjectRegistry::prime_sidecar_echo_hash for the full
        // rationale.
        if let SaveTarget::SidecarJson {
            mdl_path,
            sidecar_path,
        } = &target
            && let Err(e) = self.state.registry.prime_sidecar_echo_hash(
                mdl_path,
                sidecar_path.clone(),
                written_hash,
            )
        {
            tracing::warn!(
                error = %e,
                "MCP save: .mdl entry vanished before sidecar prime; commit_write may produce a spurious watcher merge"
            );
        }

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
                                last_diagnostic_keys: std::collections::BTreeSet::new(),
                            },
                        );
                        sidecar_path.clone()
                    }
                }
            }
            SaveTarget::InPlaceXmile(_) | SaveTarget::SdJson(_) => resolved.clone(),
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
        let rel_str = to_forward_slash(&rel);

        self.state.events.publish(WsMessage::ProjectChanged {
            path: rel_str,
            version: new_version,
            source: ChangeSource::Agent,
        });

        // Emit DiagnosticsChanged AFTER ProjectChanged when the post-merge
        // diagnostic set differs from what was cached on the registry
        // entry. Same ordering invariant as the HTTP handler: the
        // broadcast channel preserves publish order within one sender's
        // call sequence.
        crate::diagnostics::maybe_emit_diagnostics_changed(
            &self.state,
            &registry_key,
            &merged_project,
        );

        Ok(new_version)
    }

    async fn create(
        &self,
        abs_path: &Path,
        project: &datamodel::Project,
        _format: SourceFormat,
    ) -> Result<(), AccessError> {
        // The path doesn't exist yet, so we can't canonicalize it directly.
        // resolve_create_path_within_root canonicalizes the deepest
        // existing ancestor and rebuilds the path from there, rejecting
        // any `..` segment that would escape the tree.
        let resolved = resolve_create_path_within_root(&self.state, abs_path)?;

        let project_format = project_format_for_create(&resolved)?;

        if let Some(parent) = resolved.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(AccessError::WriteError)?;
        }

        let bytes = serialize_for_create(project, project_format)?;

        // Atomic exclusive create: the kernel guarantees that at most one
        // concurrent caller can pass the create_new check, so two MCP
        // CreateModel calls racing on the same path produce exactly one
        // winner and (N-1) AlreadyExists failures. The previous
        // exists()-then-atomic_write pattern was non-atomic — any racer
        // that passed exists() before another's rename completed would
        // silently overwrite the first writer's content. Same primitive
        // and crash-safety trade-off (mid-write crash leaves a partial
        // file) the HTTP `POST /api/projects/new` path uses.
        use std::io::Write as _;
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&resolved)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(AccessError::WriteError(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("file already exists: {}", resolved.display()),
                )));
            }
            Err(e) => return Err(AccessError::WriteError(e)),
        };
        if let Err(write_err) = file.write_all(&bytes) {
            // Best-effort cleanup of the partial file so subsequent
            // creates aren't blocked by an empty-or-truncated leftover.
            drop(file);
            let _ = std::fs::remove_file(&resolved);
            return Err(AccessError::WriteError(write_err));
        }
        // Sync before drop so the file's contents are durable before any
        // subsequent reader (including the watcher) sees it.
        if let Err(sync_err) = file.sync_all() {
            drop(file);
            let _ = std::fs::remove_file(&resolved);
            return Err(AccessError::WriteError(sync_err));
        }
        drop(file);
        let written_hash = content_hash(&bytes);

        // Stat the freshly-written file so the registry entry carries the
        // real on-disk size and mtime. If stat fails (vanishingly unlikely
        // since we just wrote the file), fall back to UNIX_EPOCH/byte-len
        // — the next scan or save will refresh.
        let (mtime, size) = match std::fs::metadata(&resolved) {
            Ok(m) => (
                m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                m.len(),
            ),
            Err(_) => (std::time::SystemTime::UNIX_EPOCH, bytes.len() as u64),
        };

        // upsert_max_version is the safe choice in case a concurrent
        // scanner already inserted an entry with a non-zero version.
        // For a brand-new file this is effectively the same as upsert.
        self.state.registry.upsert_max_version(
            resolved.clone(),
            ProjectMeta {
                path: PathBuf::new(),
                format: project_format,
                mtime,
                size,
                git: GitState::Untracked,
                version: 0,
                doc: Default::default(),
                last_disk_hash: written_hash,
                last_diagnostic_keys: std::collections::BTreeSet::new(),
            },
        );

        let root_canonical = self.state.root.canonicalize().map_err(|e| {
            AccessError::IoError(std::io::Error::other(format!(
                "canonicalize root for broadcast: {e}"
            )))
        })?;
        let rel = resolved
            .strip_prefix(&root_canonical)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| resolved.clone());
        let rel_str = to_forward_slash(&rel);

        self.state.events.publish(WsMessage::ProjectChanged {
            path: rel_str,
            version: 0,
            source: ChangeSource::Agent,
        });

        // Same ordering rule as the save path: ProjectChanged first,
        // then DiagnosticsChanged if the new project introduced any
        // diagnostics. A clean newly-created project produces no
        // notification (cached set is empty, computed set is empty).
        crate::diagnostics::maybe_emit_diagnostics_changed(&self.state, &resolved, project);

        Ok(())
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
            git: Arc::new(GitProbe::new_unavailable()),
            root: Arc::new(root),
            events: Arc::new(EventBus::new()),
            ui_port: 0,
            mcp_port: 0,
            strict_origin: true,
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
                last_diagnostic_keys: std::collections::BTreeSet::new(),
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
    async fn open_mdl_with_sidecar_returns_sidecar_content() {
        // HTTP `get_project` swaps a `.mdl` request to its `.sd.json`
        // sidecar when the sidecar exists on disk; MCP `open` must do
        // the same so an AI agent reading `foo.mdl` sees the same
        // content the user's editor sees. Without the swap, MCP either
        // returns NotFound (when the .mdl entry has been redirected
        // away) or the stale .mdl bytes (when a scan re-inserted the
        // .mdl entry between save and read), diverging from HTTP.
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let mdl = canonical_root.join("teacup.mdl");
        let sidecar = canonical_root.join("teacup.sd.json");
        fs::write(&mdl, b"{UTF-8}\n\nplaceholder=1\n  ~\n  ~|\n").expect("write mdl");
        let sidecar_json = r#"{"name":"sidecar-marker","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        fs::write(&sidecar, sidecar_json).expect("write sidecar");

        let state = build_state(canonical_root.clone());
        // Mirror the post-save state: the registry tracks the sidecar
        // (the .mdl entry was redirected away by the previous save).
        seed_registry(&state, &sidecar, ProjectFormat::SdJson);

        let access = RegistryAccess::new(state);
        let opened = access
            .open(&mdl)
            .await
            .expect("open must follow sidecar preference, parity with HTTP get_project");

        assert_eq!(
            opened.source_format,
            SourceFormat::NativeJson,
            "sidecar's source_format wins over the .mdl extension"
        );
        assert_eq!(
            opened.project.name, "sidecar-marker",
            "open must return the sidecar's content, not the .mdl bytes"
        );
    }

    #[tokio::test]
    async fn open_mdl_with_sidecar_prefers_sidecar_even_when_mdl_entry_present() {
        // Race-tolerant variant: even if a scan has re-inserted a
        // .mdl-keyed entry between save and read, HTTP would still
        // prefer the sidecar (its sidecar.is_file() check runs against
        // disk, not the registry). MCP must match.
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let mdl = canonical_root.join("teacup.mdl");
        let sidecar = canonical_root.join("teacup.sd.json");
        fs::write(&mdl, b"{UTF-8}\n\nplaceholder=1\n  ~\n  ~|\n").expect("write mdl");
        let sidecar_json = r#"{"name":"sidecar-marker","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        fs::write(&sidecar, sidecar_json).expect("write sidecar");

        let state = build_state(canonical_root.clone());
        // Both entries present. The .mdl entry has stale on-disk content.
        seed_registry(&state, &mdl, ProjectFormat::Mdl);
        seed_registry(&state, &sidecar, ProjectFormat::SdJson);

        let access = RegistryAccess::new(state);
        let opened = access.open(&mdl).await.expect("open succeeds");
        assert_eq!(
            opened.project.name, "sidecar-marker",
            "sidecar wins over a stale .mdl entry, parity with HTTP"
        );
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn create_is_collision_safe_under_concurrent_calls() {
        // Eight concurrent CreateModel calls targeting the same path. The
        // existing exists()-then-atomic_write pattern is non-atomic: any
        // racer that passes exists() before another's rename completes
        // overwrites the first writer's content. Exactly one create
        // must succeed; the others must surface AlreadyExists so the
        // caller can either retry with a different name or accept that
        // the file already exists.
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let target = canonical_root.join("racing.sd.json");
        let state = build_state(canonical_root.clone());
        let access = Arc::new(RegistryAccess::new(state));

        let racer_count = 8;
        let barrier = Arc::new(tokio::sync::Barrier::new(racer_count));

        let mut handles = Vec::with_capacity(racer_count);
        for i in 0..racer_count {
            let access = access.clone();
            let target = target.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                let mut project = simlin_mcp_core::types::build_empty_project();
                project.name = format!("project_{i}");
                // All racers wait at the barrier so they enter `create`
                // simultaneously, maximising the chance the buggy
                // exists()-then-write pattern shows up. With the atomic
                // create-or-fail fix exactly one passes the kernel-side
                // create_new check.
                barrier.wait().await;
                access
                    .create(&target, &project, SourceFormat::NativeJson)
                    .await
            }));
        }

        let mut successes = 0;
        let mut already_exists = 0;
        let mut other = Vec::new();
        for h in handles {
            match h.await.expect("task panicked") {
                Ok(()) => successes += 1,
                Err(AccessError::WriteError(e))
                    if e.kind() == std::io::ErrorKind::AlreadyExists =>
                {
                    already_exists += 1;
                }
                Err(other_err) => other.push(other_err),
            }
        }
        assert!(
            other.is_empty(),
            "no racer should fail with anything other than AlreadyExists; got {other:?}"
        );
        assert_eq!(
            successes, 1,
            "exactly one racer must win the create; successes={successes}, already_exists={already_exists}"
        );
        assert_eq!(
            already_exists,
            racer_count - 1,
            "all losers must surface AlreadyExists"
        );
    }
}
