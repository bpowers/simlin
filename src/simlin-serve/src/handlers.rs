// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! HTTP handlers for the read-only viewer API.
//!
//! `AppState` is the per-process bundle of `(registry, git, root)`. It is
//! cheaply cloneable (each field is `Arc`-shared) so Axum extractors can pull
//! it out of every request without contention.

use std::path::{MAIN_SEPARATOR, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::git::GitProbe;
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

/// Render a relative `PathBuf` as a string with forward-slash separators so
/// the wire format is platform-agnostic. On Unix this is a no-op cast; on
/// Windows it rewrites `\` to `/` so URLs work without further escaping.
fn path_to_forward_slash(path: &std::path::Path) -> String {
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
        assert_eq!(
            path_to_forward_slash(std::path::Path::new("a/b/c.stmx")),
            "a/b/c.stmx"
        );
    }

    #[test]
    fn forward_slash_handles_simple_relative_paths() {
        assert_eq!(
            path_to_forward_slash(std::path::Path::new("model.stmx")),
            "model.stmx"
        );
    }
}
