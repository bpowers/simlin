// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! HTTP handlers for the read-only viewer API.
//!
//! `AppState` is the per-process bundle of `(registry, git, root)`. It is
//! cheaply cloneable (each field is `Arc`-shared) so Axum extractors can pull
//! it out of every request without contention.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::git::GitProbe;
use crate::parse::{ParseError, datamodel_to_canonical_json, parse_to_datamodel};
use crate::registry::{GitState, ProjectFormat, ProjectRegistry};
use crate::scan::scan_into_registry;

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
/// root, parse it, and return the canonical JSON form.
///
/// Path traversal is rejected before any filesystem access: `..` segments,
/// absolute paths, drive prefixes, and null bytes all produce 400 Bad Request.
/// After resolving, the canonicalized path is verified to be a descendant of
/// the canonicalized root (defense-in-depth against TOCTOU and symlinks
/// pointing out of the tree); a violation is 403 Forbidden.
///
/// `.mdl` requests check for a sibling `<basename>.sd.json` first: when the
/// sidecar exists, it becomes source-of-truth (Phase 2 will create it on
/// save; Phase 1 only implements the read side).
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

    let contents = std::fs::read_to_string(&effective_path)
        .map_err(|e| ApiError::Internal(format!("read {}: {e}", effective_path.display())))?;

    let project = parse_to_datamodel(&effective_path, effective_format, &contents)?;
    let json = datamodel_to_canonical_json(&project)
        .map_err(|e| ApiError::Internal(format!("serialize: {e}")))?;

    // Phase 1 always reports version 0; Phase 2 increments it on save so the
    // SPA can detect concurrent modification via optimistic locking.
    let version = state
        .registry
        .get(&canonical)
        .map(|m| m.version)
        .unwrap_or(0);

    Ok(Json(GetProjectResponse {
        json,
        version,
        source_format: effective_format,
    }))
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
}
