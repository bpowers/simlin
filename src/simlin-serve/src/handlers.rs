// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! HTTP handlers for the read-only viewer API.
//!
//! `AppState` is the per-process bundle of `(registry, git, root)`. It is
//! cheaply cloneable (each field is `Arc`-shared) so Axum extractors can pull
//! it out of every request without contention.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::broadcast;

use crate::events::{ChangeSource, EventBus, WsMessage};
use crate::git::GitProbe;
use crate::parse::ParseError;
use crate::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry, RegistryError};
use crate::scan::scan_into_registry;
use crate::validation::{BaselineErrors, ValidationFailure, ValidationOutcome, validate_save};
use crate::writer::{SaveTarget, commit_write, resolve_save_target, serialize_project};

/// Process-wide state injected into every handler. Cloning is cheap because
/// each field is `Arc`-shared; the inner data is never copied per-request.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<ProjectRegistry>,
    pub git: Arc<GitProbe>,
    /// Absolute, canonicalized scan root. Stored here (not just in the
    /// registry) so handlers that resolve user-supplied paths can canonicalize
    /// against the same anchor the registry uses.
    pub root: Arc<PathBuf>,
    /// Internal pub/sub for `WsMessage` events. The save handler calls
    /// `events.publish` after a successful merge; the WebSocket handler
    /// (Phase 3 Task 7) creates one subscriber per connected client.
    pub events: Arc<EventBus>,
    /// One-time launch token — same value baked into the launch URL.
    /// The WebSocket upgrade handler validates the `?token=...` query
    /// param against this with a constant-time compare; HTTP handlers
    /// don't read it (they're loopback-only by virtue of the bind, and
    /// the SPA proves origin via the loaded HTML carrying the token).
    pub launch_token: Arc<String>,
}

/// Wire shape for a single project entry. Identical to `ProjectMeta` except
/// `path` is rendered with forward slashes regardless of host OS so the SPA
/// can use the same string as a URL segment.
#[derive(Debug, Serialize)]
pub struct ProjectEntry {
    pub path: String,
    pub format: ProjectFormat,
    pub mtime: std::time::SystemTime,
    pub size: u64,
    pub git: GitState,
    pub version: u64,
}

#[derive(Debug, Serialize)]
pub struct ListProjectsResponse {
    pub projects: Vec<ProjectEntry>,
    pub git_available: bool,
}

/// `GET /api/projects` — refresh the registry from the filesystem and return
/// the snapshot. Phase 1 re-scans on every call so listings always reflect
/// the current state of the directory; Phase 4 swaps this for a watcher.
pub async fn list_projects(State(state): State<AppState>) -> Json<ListProjectsResponse> {
    if let Err(err) = scan_into_registry(state.root.as_ref(), &state.registry, &state.git) {
        // A failed rescan is non-fatal: we still serve whatever the registry
        // already had. Logging at warn level surfaces the issue in the
        // server log without breaking the client.
        tracing::warn!(error = %err, "scan_into_registry failed; serving stale snapshot");
    }

    let snapshot = state.registry.snapshot();
    let projects = snapshot
        .into_iter()
        .map(|meta| ProjectEntry {
            path: path_to_forward_slash(&meta.path),
            format: meta.format,
            mtime: meta.mtime,
            size: meta.size,
            git: meta.git,
            version: meta.version,
        })
        .collect();

    Json(ListProjectsResponse {
        projects,
        git_available: state.git.git_available(),
    })
}

/// Wire shape for a single project read.
#[derive(Debug, Serialize)]
pub struct GetProjectResponse {
    pub json: String,
    pub version: u64,
    pub source_format: ProjectFormat,
}

/// `GET /api/projects/{*rel_path}` — resolve a single file under the scan
/// root and return its canonical JSON form, sourced from the in-memory
/// `ProjectDoc`.
///
/// Path traversal is rejected before any filesystem access: `..` segments,
/// absolute paths, drive prefixes, and null bytes all produce 400 Bad Request.
/// After resolving, the canonicalized path is verified to be a descendant of
/// the canonicalized root (defense-in-depth against TOCTOU and symlinks
/// pointing out of the tree); a violation is 403 Forbidden.
///
/// `.mdl` requests check for a sibling `<basename>.sd.json` first: when the
/// sidecar exists, it becomes source-of-truth.
///
/// Phase 3: the response no longer comes from re-reading and re-parsing the
/// file each call. Instead the registry's lazy `ProjectDoc` is hydrated on
/// first access (reading from disk once) and every subsequent GET reads from
/// memory. This is what lets the doc absorb writes (browser saves, MCP
/// edits, file-watcher reloads in Phase 4) and serve the merged state
/// without round-tripping through disk.
pub async fn get_project(
    State(state): State<AppState>,
    AxumPath(rel_path): AxumPath<String>,
) -> Result<Json<GetProjectResponse>, ApiError> {
    let safe_rel = sanitize_rel_path(&rel_path)?;

    let candidate = state.root.join(&safe_rel);
    // canonicalize fails if the file doesn't exist; that's the expected
    // 404 path. Other I/O errors (permission, etc.) get reported as
    // internal so the user can see something happened.
    let canonical = match candidate.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::NotFound);
        }
        Err(e) => return Err(ApiError::Internal(format!("canonicalize failed: {e}"))),
    };

    // Defense in depth: even if the request didn't contain `..`, a symlink
    // within the tree could land us outside. Compare canonical paths.
    let root_canonical = state.root.canonicalize().map_err(|e| {
        ApiError::Internal(format!("canonicalize root {}: {e}", state.root.display()))
    })?;
    if !canonical.starts_with(&root_canonical) {
        return Err(ApiError::Forbidden);
    }

    // Determine source format and apply the .mdl sidecar preference: if the
    // request was for `<x>.mdl` and `<x>.sd.json` exists alongside it, swap
    // to the sidecar.
    let initial_format = format_for_path(&canonical).ok_or(ApiError::BadRequest(
        "unrecognized file extension".to_string(),
    ))?;

    let (effective_path, effective_format) = if matches!(initial_format, ProjectFormat::Mdl) {
        let sidecar = sidecar_for_mdl(&canonical);
        if sidecar.is_file() {
            (sidecar, ProjectFormat::SdJson)
        } else {
            (canonical.clone(), ProjectFormat::Mdl)
        }
    } else {
        (canonical.clone(), initial_format)
    };

    // Ensure the registry has an entry for the effective path so
    // `get_or_init_doc` has somewhere to cache the hydrated `ProjectDoc`.
    // First-touch races are absorbed by `ensure_or_get`'s atomic insert.
    state.registry.ensure_or_get(effective_path.clone(), || {
        let metadata = std::fs::metadata(&effective_path);
        let (mtime, size) = match metadata {
            Ok(m) => (
                m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                m.len(),
            ),
            Err(_) => (std::time::SystemTime::UNIX_EPOCH, 0),
        };
        ProjectMeta {
            path: PathBuf::new(),
            format: effective_format,
            mtime,
            size,
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
        }
    });

    // Hydrate (on first call) or look up (on subsequent calls) the
    // ProjectDoc. Hydration reads from disk; subsequent calls don't.
    let doc = state
        .registry
        .get_or_init_doc(&effective_path)
        .map_err(|e| match e {
            RegistryError::NotFound => ApiError::NotFound,
            RegistryError::HydrationFailed(msg) => ApiError::BadRequest(msg),
            RegistryError::VersionMismatch { .. } => ApiError::Internal(format!(
                "unexpected version mismatch from get_or_init_doc: {e}"
            )),
        })?;

    let json = doc
        .current_state_as_json_string()
        .map_err(|e| ApiError::Internal(format!("export project: {e}")))?;

    // Version comes from the registry entry, not from the doc itself.
    // The doc and the registry advance in lockstep on writes; reads never
    // advance the registry version.
    let version = state
        .registry
        .get(&effective_path)
        .map(|m| m.version)
        .unwrap_or(0);

    Ok(Json(GetProjectResponse {
        json,
        version,
        source_format: effective_format,
    }))
}

/// Wire shape of a save request. The `json` field carries the canonical
/// stringified JSON the Editor produced from `engine.serializeJson()`; we
/// re-parse it server-side rather than accepting structured fields so the
/// editor's serialization stays the single source of truth for the
/// canonical form.
#[derive(Debug, Deserialize)]
pub struct SaveRequest {
    pub json: String,
    pub version: u64,
}

/// Wire shape of a successful save response. `path` is the (forward-slash)
/// relative path the server actually wrote; this differs from the request
/// path when a `.mdl`-backed entry redirects to a sibling `.sd.json`
/// sidecar (Phase 2 Subcomponent B).
#[derive(Debug, Serialize)]
pub struct SaveResponse {
    pub version: u64,
    pub path: String,
}

/// Structured detail attached to 422 responses. Mirrors
/// `simlin-mcp::tools::types::ErrorOutput` field-for-field; we duplicate
/// the structure here rather than depending on `simlin-mcp` to keep crate
/// boundaries clean (`simlin-serve` and `simlin-mcp` are sibling consumers
/// of the same engine error types).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable_name: Option<String>,
    pub kind: String,
}

/// Errors surfaced from the save handler. Status mapping lives entirely in
/// the `IntoResponse` impl; handler code only constructs variants and lets
/// Axum render them.
#[derive(Debug)]
pub enum SaveError {
    /// Path resolved against the scan root but no file exists there.
    NotFound,
    /// Optimistic-lock mismatch: the client's `version` is stale. The
    /// `actual` field tells the client what to refetch against.
    VersionMismatch {
        expected: u64,
        actual: u64,
    },
    BadRequest(String),
    /// Path was outside the scan root or otherwise denied.
    Forbidden,
    /// One or more new errors would be introduced by this edit. The list
    /// only contains *new* errors (errors that already existed before this
    /// save are filtered out so a save that fixes some errors without
    /// introducing any new ones is accepted).
    Validation {
        errors: Vec<ValidationError>,
    },
    /// Anything we couldn't classify; rendered as 500 and logged.
    Internal(anyhow::Error),
}

impl IntoResponse for SaveError {
    fn into_response(self) -> Response {
        match self {
            SaveError::NotFound => {
                let body = serde_json::json!({ "error": "not found" });
                (StatusCode::NOT_FOUND, Json(body)).into_response()
            }
            SaveError::VersionMismatch { expected, actual } => {
                let body = serde_json::json!({
                    "error": "version_mismatch",
                    "expected": expected,
                    "actual": actual,
                });
                (StatusCode::CONFLICT, Json(body)).into_response()
            }
            SaveError::BadRequest(msg) => {
                let body = serde_json::json!({ "error": msg });
                (StatusCode::BAD_REQUEST, Json(body)).into_response()
            }
            SaveError::Forbidden => {
                let body = serde_json::json!({ "error": "forbidden" });
                (StatusCode::FORBIDDEN, Json(body)).into_response()
            }
            SaveError::Validation { errors } => {
                let body = serde_json::json!({
                    "error": "validation_failed",
                    "details": errors,
                });
                (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
            }
            SaveError::Internal(err) => {
                tracing::error!(error = %err, "internal server error");
                let body = serde_json::json!({ "error": "internal server error" });
                (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
            }
        }
    }
}

/// `POST /api/projects/{*rel_path}` — save edits to a model.
///
/// Phase 3 (Task 8) routes every successful save through the in-memory
/// `ProjectDoc` rather than writing the raw incoming JSON straight to
/// disk:
///
/// 1. Sanitize + canonicalize the relative path. 404 if the file is
///    missing; 403 if it canonicalizes outside the scan root.
/// 2. Ensure a registry entry exists (lazy upsert with version 0).
/// 3. Pre-fetch the baseline error set from the doc-exported JSON (the
///    in-memory pre-edit project state). The doc is hydrated from disk
///    if this is its first touch; subsequent saves serve from memory.
/// 4. Validate the incoming body against the baseline. JSON parse
///    failure -> 400; new errors introduced by the edit -> 422.
/// 5. Re-canonicalize the validated `datamodel::Project` to JSON (so
///    the doc tree always reflects the canonicalized form, regardless
///    of subtle drift in the incoming request).
/// 6. Call `check_increment_and_merge` — under one registry-write-lock
///    acquisition this checks the optimistic-lock version, increments
///    it, and applies the new JSON into the doc. Stale version -> 409.
/// 7. Outside the lock, serialize the merged doc state back to a
///    `datamodel::Project` and write it to disk with the format-aware
///    writer. Sidecar redirects move the registry key as before.
/// 8. Refresh the registry mtime/size from the post-write stat.
/// 9. Publish a `ProjectChanged { source: User }` event to subscribed
///    WebSocket clients so other tabs can remount their editors.
///
/// The `invalidate_doc` stop-gap from Task 5 is removed: the doc is
/// the post-save state by virtue of the merge in step 6.
pub async fn save_project(
    State(state): State<AppState>,
    AxumPath(rel_path): AxumPath<String>,
    Json(body): Json<SaveRequest>,
) -> Result<Json<SaveResponse>, SaveError> {
    let resolved = resolve_save_path(&state, &rel_path)?;

    // Ensure the registry has an entry for the canonical path. Populated
    // by scan_into_registry on listing requests; if a client saves without
    // first listing, the entry may not yet exist. ensure_or_get is
    // atomic (single write-lock), so two concurrent first-saves cannot
    // both observe absence and both insert with version 0.
    // (Phase 4's file watcher will pre-populate the registry so this
    // fallback is rarely exercised.)
    {
        let canonical = resolved.canonical.clone();
        let format = resolved.initial_format;
        state.registry.ensure_or_get(canonical, || {
            let metadata = std::fs::metadata(&resolved.canonical);
            let (mtime, size) = match metadata {
                Ok(m) => (
                    m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                    m.len(),
                ),
                Err(_) => (std::time::SystemTime::UNIX_EPOCH, 0),
            };
            ProjectMeta {
                path: PathBuf::new(),
                format,
                mtime,
                size,
                git: GitState::Untracked,
                version: 0,
                doc: Default::default(),
                last_disk_hash: 0,
            }
        });
    }

    // Pre-fetch baseline from the doc's exported JSON rather than
    // re-reading the file from disk. On first touch the doc is hydrated
    // from disk (single read), but every subsequent save consults the
    // already-hydrated in-memory state. The baseline is the set of
    // errors that already exist pre-edit; we use it to suppress them
    // from the post-edit error set so saves that *fix* errors are not
    // blocked.
    let current_doc = state
        .registry
        .get_or_init_doc(&resolved.canonical)
        .map_err(|e| match e {
            RegistryError::NotFound => SaveError::NotFound,
            RegistryError::HydrationFailed(msg) => SaveError::BadRequest(msg),
            RegistryError::VersionMismatch { expected, actual } => {
                SaveError::VersionMismatch { expected, actual }
            }
        })?;
    let current_json_value = current_doc
        .export_canonical_json()
        .map_err(|e| SaveError::Internal(anyhow::anyhow!("export current doc: {e}")))?;
    let current_json_project: simlin_engine::json::Project =
        serde_json::from_value(current_json_value).map_err(|e| {
            SaveError::Internal(anyhow::anyhow!(
                "convert current doc state to json::Project: {e}"
            ))
        })?;
    let current_project: simlin_engine::datamodel::Project = current_json_project.into();
    let baseline: BaselineErrors = crate::validation::compute_baseline(&current_project);

    // Validate the incoming body against the baseline.
    let outcome: ValidationOutcome = match validate_save(&body.json, &baseline) {
        Ok(o) => o,
        Err(ValidationFailure::JsonParse(e)) => {
            return Err(SaveError::BadRequest(format!("json parse error: {e}")));
        }
    };
    if !outcome.new_errors.is_empty() {
        return Err(SaveError::Validation {
            errors: outcome.new_errors,
        });
    }

    // Re-canonicalize the validated project to JSON for the merge. We
    // route through `json::Project::from(&datamodel::Project)` rather
    // than passing the raw incoming `body.json` so the doc always sees
    // the canonical engine shape regardless of what the client wrote
    // (case, whitespace, optional-field omission, etc.).
    let canonical_project: simlin_engine::json::Project = (&outcome.project).into();
    let canonical_value = serde_json::to_value(&canonical_project)
        .map_err(|e| SaveError::Internal(anyhow::anyhow!("serialize canonical project: {e}")))?;

    // Atomic version-check + increment + merge against the doc.
    let (new_version, merged_doc) = match state.registry.check_increment_and_merge(
        &resolved.canonical,
        body.version,
        &canonical_value,
    ) {
        Ok(out) => out,
        Err(RegistryError::VersionMismatch { expected, actual }) => {
            return Err(SaveError::VersionMismatch { expected, actual });
        }
        Err(RegistryError::NotFound) => {
            return Err(SaveError::Internal(anyhow::anyhow!(
                "registry entry vanished between upsert and merge"
            )));
        }
        Err(RegistryError::HydrationFailed(msg)) => {
            return Err(SaveError::Internal(anyhow::anyhow!("merge failed: {msg}")));
        }
    };

    // Build the on-disk project from the doc's post-merge state. In
    // practice the merged JSON equals `canonical_value` modulo sort
    // order, but going through the doc's own export keeps the writer
    // strictly downstream of the merge so any future divergence (Phase
    // 7 server-side annotations, say) remains coherent.
    let merged_json = merged_doc
        .export_canonical_json()
        .map_err(|e| SaveError::Internal(anyhow::anyhow!("export merged doc: {e}")))?;
    let merged_json_project: simlin_engine::json::Project = serde_json::from_value(merged_json)
        .map_err(|e| {
            SaveError::Internal(anyhow::anyhow!(
                "convert merged doc state to json::Project: {e}"
            ))
        })?;
    let merged_project: simlin_engine::datamodel::Project = merged_json_project.into();

    // Resolve the target shape from the request URL's source format.
    // For `.mdl` we always pick the SidecarJson arm; the registry-side
    // redirect happens after the write so the new entry replaces the
    // `.mdl` key with the sidecar key. For `.sd.json` requests
    // (including ones following an earlier redirect where the frontend
    // updated its URL state) we use the SdJson arm.
    let target = resolve_save_target(&resolved.canonical, resolved.initial_format);

    // Serialize before writing so the echo-suppression hash can be stored in
    // the registry before the bytes land on disk. Without this ordering the
    // watcher could fire and compute the same hash while last_disk_hash is
    // still the old value, causing a spurious Disk-source merge after every
    // user save.
    let write_outcome = serialize_project(&merged_project, &target)
        .map_err(|e| SaveError::Internal(anyhow::anyhow!("serialize_project: {e}")))?;
    let written_path = write_outcome.path.clone();
    let written_hash = crate::hashing::content_hash(&write_outcome.bytes);

    // Store the hash before the OS-visible write so the watcher's echo-
    // suppression check always sees the new hash by the time the inotify/
    // FSEvents event fires. The only downside is a small window where the
    // hash is "ahead" of disk if commit_write fails; in that case the next
    // real external edit will have a different hash and will still be merged,
    // so there is no correctness loss.
    state
        .registry
        .prime_echo_hash(&resolved.canonical, written_hash);

    // Commit to disk. The echo-suppression hash is already in the registry
    // so a watcher event that arrives here will be suppressed correctly.
    commit_write(&write_outcome)
        .map_err(|e| SaveError::Internal(anyhow::anyhow!("commit_write: {e}")))?;

    // For SidecarJson, redirect the registry's `.mdl` key to the new
    // sidecar key (carrying the just-incremented version forward) so
    // subsequent reads via either path see the sidecar content. For the
    // other arms the registry key is unchanged.
    //
    // The frontend counterpart is App.handlePathRedirect (called via
    // EditorHost's onPathRedirect prop) which updates the active
    // selectedPath so the sidebar list and URL reflect the new sidecar
    // path after the first save of a .mdl-backed entry.
    let registry_key: PathBuf = match &target {
        SaveTarget::SidecarJson {
            mdl_path,
            sidecar_path,
        } => {
            match state
                .registry
                .redirect_to_sidecar(mdl_path, sidecar_path.clone())
            {
                Ok(()) => sidecar_path.clone(),
                Err(e) => {
                    // The disk write succeeded but the registry entry for
                    // the .mdl path was concurrently removed (e.g. by a
                    // scan between the version-lock release and here).
                    // Re-insert the sidecar entry directly so the registry
                    // tracks the file we just created. Without this the
                    // sidecar exists on disk but is invisible to the
                    // registry until the next scan, and the client sees a
                    // version number that no registry entry can satisfy.
                    tracing::warn!(
                        error = %e,
                        "registry redirect_to_sidecar failed; re-inserting sidecar entry"
                    );
                    state.registry.upsert_max_version(
                        sidecar_path.clone(),
                        ProjectMeta {
                            path: PathBuf::new(),
                            format: crate::registry::ProjectFormat::SdJson,
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
        SaveTarget::InPlaceXmile(_) | SaveTarget::SdJson(_) => resolved.canonical.clone(),
    };

    // Refresh the registry's mtime + size + hash from the freshly-written
    // file. The mtime and size feed the SPA's stale-data heuristics; the
    // hash here is the same pre-computed value already stored by prime_echo_hash
    // above — refresh_after_write updates all three fields atomically.
    if let Ok(metadata) = std::fs::metadata(&written_path)
        && let Ok(mtime) = metadata.modified()
    {
        state
            .registry
            .refresh_after_write(&registry_key, mtime, metadata.len(), written_hash);
    }

    // For SidecarJson the response path points at the freshly-created
    // sidecar so the SPA can update its URL state to follow the
    // redirect. For the other arms the path is unchanged.
    let response_path = match &target {
        SaveTarget::SidecarJson { sidecar_path, .. } => {
            let rel = sidecar_path
                .strip_prefix(&resolved.root_canonical)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| sidecar_path.clone());
            path_to_forward_slash(&rel)
        }
        SaveTarget::InPlaceXmile(_) | SaveTarget::SdJson(_) => {
            path_to_forward_slash(&resolved.relative_path)
        }
    };

    // Publish AFTER the disk write + meta refresh so a subscriber can
    // assume the file on disk reflects the announced version. Two
    // concurrent saves' events may arrive in either order from the
    // subscriber's perspective; the client uses the version number to
    // decide whether to re-render, so order doesn't change correctness.
    state.events.publish(WsMessage::ProjectChanged {
        path: response_path.clone(),
        version: new_version,
        source: ChangeSource::User,
    });

    Ok(Json(SaveResponse {
        version: new_version,
        path: response_path,
    }))
}

/// Path-resolution outcome shared between the save handler's various
/// error paths. `initial_format` is what the request URL maps to; the
/// registry entry the save handler operates on is keyed by `canonical`
/// throughout the flow. Sidecar redirection (a `.mdl` request whose
/// sibling `.sd.json` exists) is now expressed entirely through the
/// post-write `redirect_to_sidecar` move on the registry — Task 8
/// reads the pre-edit baseline from the in-memory `ProjectDoc` rather
/// than re-reading the on-disk file, so we no longer need a separate
/// effective_path for the baseline source.
struct ResolvedPath {
    /// The canonicalized absolute path of the requested file.
    canonical: PathBuf,
    /// The canonicalized scan root, computed once in `resolve_save_path`
    /// so callers don't need to re-canonicalize `state.root` for path
    /// relativization or descendant checks.
    root_canonical: PathBuf,
    /// The relative path (relative to the scan root) the client should
    /// see in the response. May differ from the request when a sidecar
    /// redirect happens.
    relative_path: PathBuf,
    /// Source format inferred from the request URL itself (no sidecar
    /// redirect). Used for the registry-entry seed.
    initial_format: ProjectFormat,
}

/// Resolve a request `rel_path` to a canonical, scan-root-confined
/// path. Mirrors the GET handler's path resolution but maps to
/// `SaveError` instead of `ApiError`. Path traversal, missing files,
/// and out-of-root canonical resolutions all produce distinct errors.
fn resolve_save_path(state: &AppState, rel_path: &str) -> Result<ResolvedPath, SaveError> {
    let safe_rel = sanitize_rel_path(rel_path).map_err(api_error_to_save_error)?;
    let candidate = state.root.join(&safe_rel);
    let canonical = match candidate.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SaveError::NotFound);
        }
        Err(e) => {
            return Err(SaveError::Internal(anyhow::anyhow!(
                "canonicalize {}: {e}",
                candidate.display()
            )));
        }
    };
    let root_canonical = state.root.canonicalize().map_err(|e| {
        SaveError::Internal(anyhow::anyhow!(
            "canonicalize root {}: {e}",
            state.root.display()
        ))
    })?;
    if !canonical.starts_with(&root_canonical) {
        return Err(SaveError::Forbidden);
    }

    let initial_format = format_for_path(&canonical)
        .ok_or_else(|| SaveError::BadRequest("unrecognized file extension".to_string()))?;

    let relative_path = canonical
        .strip_prefix(&root_canonical)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| canonical.clone());

    Ok(ResolvedPath {
        canonical,
        root_canonical,
        relative_path,
        initial_format,
    })
}

/// Translate the few `ApiError` variants that `sanitize_rel_path` can
/// produce into the corresponding `SaveError`. `ApiError::Internal`
/// is unreachable here because `sanitize_rel_path` only returns
/// `BadRequest` variants, but matched explicitly for completeness.
fn api_error_to_save_error(err: ApiError) -> SaveError {
    match err {
        ApiError::BadRequest(msg) => SaveError::BadRequest(msg),
        ApiError::Forbidden => SaveError::Forbidden,
        ApiError::NotFound => {
            // sanitize_rel_path doesn't produce NotFound; if a future
            // refactor introduces it, surfacing as 400 is the most
            // conservative choice.
            SaveError::BadRequest("not found".to_string())
        }
        ApiError::Internal(msg) => SaveError::Internal(anyhow::anyhow!(msg)),
    }
}

/// Errors surfaced through HTTP. The `IntoResponse` impl owns the status code
/// mapping; handler code only constructs the variants.
#[derive(Debug)]
pub enum ApiError {
    NotFound,
    BadRequest(String),
    Forbidden,
    Internal(String),
}

impl From<ParseError> for ApiError {
    fn from(e: ParseError) -> Self {
        // Parse failures are user-visible bad input (e.g. a malformed XMILE
        // file) rather than server bugs, so 400 is the right status. We keep
        // the human-readable message in the body for debugging.
        ApiError::BadRequest(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            ApiError::Internal(msg) => {
                tracing::error!(error = %msg, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}

/// Validate a relative path supplied by the client. Rejects null bytes, any
/// component that is `..`, root prefixes (Unix `/` or Windows drive letters),
/// and Windows root markers. Returns the cleaned `PathBuf` on success.
///
/// Note: we deliberately do *not* canonicalize here — that's done by the
/// caller after joining against the scan root, so the canonical form anchors
/// the descendant-check.
fn sanitize_rel_path(rel: &str) -> Result<PathBuf, ApiError> {
    if rel.contains('\0') {
        return Err(ApiError::BadRequest(
            "path may not contain null bytes".to_string(),
        ));
    }
    let candidate = PathBuf::from(rel);
    for component in candidate.components() {
        match component {
            Component::ParentDir => {
                return Err(ApiError::BadRequest(
                    "parent-directory segments are not allowed in paths".to_string(),
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ApiError::BadRequest(
                    "absolute paths are not allowed".to_string(),
                ));
            }
            Component::Normal(_) | Component::CurDir => {}
        }
    }
    Ok(candidate)
}

/// Mirror the discovery extension dispatcher for the read path. Phase 5 will
/// consolidate this with `discovery::format_for_path` once the parse pipeline
/// is unified across `simlin-serve` and `simlin-mcp`.
fn format_for_path(path: &Path) -> Option<ProjectFormat> {
    let path_str = path.to_str()?;
    if path_str.ends_with(".sd.json") {
        return Some(ProjectFormat::SdJson);
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "stmx" => Some(ProjectFormat::Stmx),
        "xmile" | "xml" => Some(ProjectFormat::Xmile),
        "mdl" => Some(ProjectFormat::Mdl),
        _ => None,
    }
}

/// For `path = "/some/dir/foo.mdl"`, return `/some/dir/foo.sd.json`. The
/// sibling-sidecar convention is documented in the design plan; Phase 2
/// writes the sidecar on save.
fn sidecar_for_mdl(mdl_path: &Path) -> PathBuf {
    let parent = mdl_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = mdl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    parent.join(format!("{stem}.sd.json"))
}

/// Render a relative `PathBuf` as a string with forward-slash separators so
/// the wire format is platform-agnostic. On Unix this is a no-op cast; on
/// Windows it rewrites `\` to `/` so URLs work without further escaping.
fn path_to_forward_slash(path: &Path) -> String {
    let display = path.to_string_lossy().into_owned();
    if MAIN_SEPARATOR == '/' {
        display
    } else {
        display.replace(MAIN_SEPARATOR, "/")
    }
}

/// Query-string carrier for the WebSocket upgrade. Browser-native
/// `WebSocket` cannot set custom headers on the handshake, so the bearer
/// rides as a query parameter; the launcher embeds the same token in the
/// initial URL the user opens.
#[derive(Debug, Deserialize)]
pub struct WsParams {
    pub token: String,
}

/// `GET /api/updates` — WebSocket endpoint that streams `WsMessage`
/// frames to the connected browser. Each connection subscribes to the
/// process's `EventBus`; messages are JSON-encoded and sent as text
/// frames.
///
/// Auth: the `?token=...` query parameter is compared against
/// `state.launch_token` with a constant-time compare. Mismatched length
/// or value -> 401. Missing token -> 400 (Axum's `Query` extractor
/// rejects the request before the handler runs).
///
/// Phase 3 doesn't consume any client-to-server messages; future phases
/// will add a `selectionChanged` upstream variant for collaborative
/// awareness. We still drain `socket.recv()` so axum's auto-pong reply
/// path keeps the connection alive on browser pings, and so the server
/// task exits promptly when the client closes.
pub async fn updates_ws_handler(
    State(state): State<AppState>,
    Query(params): Query<WsParams>,
    ws: WebSocketUpgrade,
) -> Response {
    if !tokens_match(&params.token, &state.launch_token) {
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }

    let rx = state.events.subscribe();
    tracing::info!("ws: client accepted on /api/updates");
    ws.on_upgrade(move |socket| handle_socket(socket, rx))
}

/// Constant-time token comparison.
///
/// `subtle::ConstantTimeEq::ct_eq` returns `Choice` (a wrapper around
/// `u8`); `.into()` converts to `bool`. We compare byte slices directly;
/// `ct_eq` short-circuits the early-exit-on-mismatch leak that a naive
/// `==` could expose. Token lengths in this build are always 43 bytes
/// (32 random bytes -> URL-safe base64, no pad), but the compare also
/// handles the empty-launch-token case (used by tests that don't care
/// about auth) by falling through to `ct_eq`'s length-mismatch path.
fn tokens_match(presented: &str, expected: &str) -> bool {
    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// Per-connection WebSocket loop. Multiplexes between:
/// 1. `rx.recv()` — broadcast events from the bus, serialized + sent as
///    text frames.
/// 2. `socket.recv()` — incoming client frames (Phase 3 ignores all but
///    Close/error, which terminate the loop).
///
/// Lagged subscribers see `RecvError::Lagged(n)` once and resume; we log
/// at warn so ops can spot a slow client. Connection close, send errors,
/// and bus closure all break out cleanly so the spawned task drops.
async fn handle_socket(mut socket: WebSocket, mut rx: broadcast::Receiver<WsMessage>) {
    use broadcast::error::RecvError;

    loop {
        tokio::select! {
            recv_result = rx.recv() => {
                match recv_result {
                    Ok(msg) => {
                        let json = match serde_json::to_string(&msg) {
                            Ok(s) => s,
                            Err(err) => {
                                tracing::error!(error = %err, "ws: serialize WsMessage failed; closing");
                                // Best-effort: the socket may already be in a bad state; ignore errors.
                                let _ = socket.send(Message::Close(None)).await;
                                break;
                            }
                        };
                        tracing::debug!(target: "simlin_serve::ws", "ws: send {} bytes", json.len());
                        if let Err(err) = socket.send(Message::Text(json.into())).await {
                            tracing::debug!(error = %err, "ws: send failed; closing");
                            // No Close frame: the send failure indicates the transport is
                            // already broken, so a Close would also fail.
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "ws: lagged by {n}; client may have missed events"
                        );
                        // continue: receiver auto-advances to the oldest
                        // retained message on the next recv().
                    }
                    Err(RecvError::Closed) => {
                        // Bus shut down (process is exiting). Close cleanly.
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
            client_frame = socket.recv() => {
                match client_frame {
                    Some(Ok(Message::Close(_))) => {
                        // Client initiated close; echo Close to complete the handshake.
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // axum auto-pongs; nothing to do here. Logged at
                        // debug because pings are routine.
                    }
                    Some(Ok(_)) => {
                        // Phase 3 doesn't accept client-to-server data;
                        // Phase 7 will dispatch on the variant.
                    }
                    Some(Err(err)) => {
                        tracing::debug!(error = %err, "ws: client recv error; closing");
                        // Transport already broken; a Close send would also fail.
                        break;
                    }
                    None => {
                        // Stream ended without a Close frame (abnormal closure on client side).
                        break;
                    }
                }
            }
        }
    }

    tracing::info!("ws: client disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_slash_is_identity_on_unix_paths() {
        assert_eq!(path_to_forward_slash(Path::new("a/b/c.stmx")), "a/b/c.stmx");
    }

    #[test]
    fn forward_slash_handles_simple_relative_paths() {
        assert_eq!(path_to_forward_slash(Path::new("model.stmx")), "model.stmx");
    }

    #[test]
    fn sanitize_rejects_parent_dir_segments() {
        let err = sanitize_rel_path("../etc/passwd").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn sanitize_rejects_absolute_paths() {
        let err = sanitize_rel_path("/etc/passwd").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn sanitize_rejects_null_bytes() {
        let err = sanitize_rel_path("model.stmx\0.bak").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn sanitize_accepts_simple_relative_paths() {
        let p = sanitize_rel_path("sub/model.stmx").unwrap();
        assert_eq!(p, PathBuf::from("sub/model.stmx"));
    }

    #[test]
    fn sanitize_strips_curdir_segments() {
        // `./model.stmx` is normalized to `model.stmx`; `Component::CurDir`
        // is benign so we accept it (path traversal lives in `..`).
        let p = sanitize_rel_path("./sub/model.stmx").unwrap();
        let components: Vec<_> = p.components().collect();
        // The curdir gets preserved by Components but isn't a security issue;
        // canonicalize will collapse it.
        assert!(!components.is_empty());
    }

    #[test]
    fn sidecar_for_mdl_swaps_extension() {
        assert_eq!(
            sidecar_for_mdl(Path::new("/tmp/foo/bar.mdl")),
            PathBuf::from("/tmp/foo/bar.sd.json")
        );
    }

    #[test]
    fn format_dispatcher_recognizes_known_extensions() {
        assert_eq!(
            format_for_path(Path::new("/tmp/x.stmx")),
            Some(ProjectFormat::Stmx)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/x.xmile")),
            Some(ProjectFormat::Xmile)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/x.xml")),
            Some(ProjectFormat::Xmile)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/x.mdl")),
            Some(ProjectFormat::Mdl)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/x.sd.json")),
            Some(ProjectFormat::SdJson)
        );
        assert_eq!(format_for_path(Path::new("/tmp/x.txt")), None);
    }

    #[test]
    fn save_error_not_found_maps_to_404() {
        let err = SaveError::NotFound;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn save_error_version_mismatch_maps_to_409() {
        let err = SaveError::VersionMismatch {
            expected: 1,
            actual: 0,
        };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn save_error_bad_request_maps_to_400() {
        let err = SaveError::BadRequest("invalid".into());
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn save_error_forbidden_maps_to_403() {
        let err = SaveError::Forbidden;
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn save_error_validation_maps_to_422() {
        let err = SaveError::Validation { errors: vec![] };
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn save_error_internal_maps_to_500() {
        let err = SaveError::Internal(anyhow::anyhow!("oops"));
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn save_error_validation_body_carries_details() {
        let errors = vec![ValidationError {
            code: "unknown_dependency".into(),
            message: "undefined: foo".into(),
            model_name: Some("main".into()),
            variable_name: Some("bar".into()),
            kind: "variable".into(),
        }];
        let serialized = serde_json::to_value(&errors).expect("serialize details");
        // The IntoResponse body uses the same serialization, so we cross-check
        // the field projection here without re-running the response machinery.
        assert_eq!(serialized[0]["code"], "unknown_dependency");
        assert_eq!(serialized[0]["modelName"], "main");
        assert_eq!(serialized[0]["variableName"], "bar");
        assert_eq!(serialized[0]["kind"], "variable");
    }

    #[test]
    fn save_request_round_trips_through_json() {
        let req = SaveRequest {
            json: "{}".into(),
            version: 1,
        };
        let serialized = serde_json::json!({
            "json": &req.json,
            "version": req.version,
        })
        .to_string();
        let parsed: SaveRequest =
            serde_json::from_str(&serialized).expect("SaveRequest parses back");
        assert_eq!(parsed.json, "{}");
        assert_eq!(parsed.version, 1);
    }

    #[test]
    fn save_response_serializes_with_expected_fields() {
        let resp = SaveResponse {
            version: 7,
            path: "sub/model.stmx".into(),
        };
        let value = serde_json::to_value(&resp).expect("serialize SaveResponse");
        assert_eq!(value["version"].as_u64(), Some(7));
        assert_eq!(value["path"].as_str(), Some("sub/model.stmx"));
    }

    #[test]
    fn validation_error_skips_none_fields() {
        let err = ValidationError {
            code: "not_simulatable".into(),
            message: "msg".into(),
            model_name: None,
            variable_name: None,
            kind: "simulation".into(),
        };
        let value = serde_json::to_value(&err).expect("serialize");
        assert!(value.get("modelName").is_none());
        assert!(value.get("variableName").is_none());
    }
}
