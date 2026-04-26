// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! `ListProjects` MCP tool — mirrors `GET /api/projects` for AI clients.
//!
//! The HTTP endpoint returns `mtime` and `size` so the SPA can render the
//! file listing as a directory tree; AI clients don't render file pickers,
//! so we drop those fields here to keep token counts small. The wire shape
//! is otherwise identical: one entry per registry snapshot row, with format
//! and git status surfaced verbatim.

use std::path::{MAIN_SEPARATOR, Path};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::handlers::AppState;
use crate::registry::{GitState, ProjectFormat};
use crate::scan::scan_into_registry;

/// Mirror of [`crate::registry::GitState`] shaped for MCP output.
///
/// The registry's type can't derive `schemars::JsonSchema` without
/// propagating the dependency through the entire library; mapping into
/// this local form keeps the dependency boundary clean and gives the AI
/// schema a stable name. The `kind`-discriminated wire shape is identical
/// to the registry's `Serialize` impl so the JSON surface matches what
/// the SPA already sees.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GitStateOutput {
    Tracked { dirty: bool },
    Untracked,
    Unavailable,
}

impl From<GitState> for GitStateOutput {
    fn from(g: GitState) -> Self {
        match g {
            GitState::Tracked { dirty } => GitStateOutput::Tracked { dirty },
            GitState::Untracked => GitStateOutput::Untracked,
            GitState::Unavailable => GitStateOutput::Unavailable,
        }
    }
}

/// Input for the `ListProjects` tool — no fields. We still derive
/// `JsonSchema` so the rmcp macro can produce the empty-object schema
/// the MCP wire format expects.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListProjectsInput {}

/// One entry in the projects list.
///
/// `path` is the relative-to-root form with forward slashes (so AI clients
/// can pass it back as-is to other tools regardless of host OS). `format`
/// is rendered as the registry's lowercase `Display` form so wire output
/// stays consistent with `GET /api/projects`'s discriminator.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummary {
    pub path: String,
    pub format: String,
    pub git: GitStateOutput,
    pub version: u64,
}

/// Output for the `ListProjects` tool.
///
/// `root` is the absolute working directory so the AI knows where it's
/// operating without having to round-trip through `get_info`'s
/// instructions string. `git_available` lets the AI distinguish between
/// "git is installed but this file is untracked" and "git is missing
/// from PATH altogether".
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListProjectsOutput {
    pub projects: Vec<ProjectSummary>,
    pub git_available: bool,
    pub root: String,
}

/// Compute the response. Rescans before returning so the snapshot
/// reflects on-disk additions/removals since the last call (Phase 1's
/// rescan-on-each-request semantics — Phase 4's watcher already keeps
/// the registry warm, but a fresh scan is cheap and forgiving when an
/// agent operates on a directory that hasn't been watched).
pub fn run(state: &Arc<AppState>) -> ListProjectsOutput {
    if let Err(err) = scan_into_registry(state.root.as_ref(), &state.registry, &state.git) {
        // A failed rescan is non-fatal: serve whatever the registry
        // already had. Same policy as the HTTP `list_projects` handler.
        tracing::warn!(error = %err, "ListProjects: scan_into_registry failed; serving stale snapshot");
    }

    let snapshot = state.registry.snapshot();
    let projects = snapshot
        .into_iter()
        .map(|meta| ProjectSummary {
            path: path_to_forward_slash(&meta.path),
            format: format_to_string(meta.format),
            git: meta.git.into(),
            version: meta.version,
        })
        .collect();

    ListProjectsOutput {
        projects,
        git_available: state.git.git_available(),
        root: state.root.display().to_string(),
    }
}

/// Render a relative path as forward-slash-separated UTF-8 so the wire
/// format is platform-agnostic.
fn path_to_forward_slash(path: &Path) -> String {
    let display = path.to_string_lossy().into_owned();
    if MAIN_SEPARATOR == '/' {
        display
    } else {
        display.replace(MAIN_SEPARATOR, "/")
    }
}

/// Render a `ProjectFormat` as a lowercase string. `ProjectFormat`'s
/// `Serialize` impl uses `rename_all = "snake_case"`, but the registry
/// `Display` impl uses the same form ("stmx", "xmile", "mdl", "sd_json"),
/// so we just defer to `Display`.
fn format_to_string(format: ProjectFormat) -> String {
    format.to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::SystemTime;

    use tempfile::TempDir;

    use super::*;
    use crate::events::EventBus;
    use crate::git::GitProbe;
    use crate::registry::{ProjectMeta, ProjectRegistry};

    fn build_state(root: PathBuf) -> Arc<AppState> {
        Arc::new(AppState {
            registry: Arc::new(ProjectRegistry::new(root.clone())),
            git: Arc::new(GitProbe::new_unavailable()),
            root: Arc::new(root),
            events: Arc::new(EventBus::new()),
            launch_token: Arc::new(String::new()),
            ui_port: 0,
            mcp_port: 0,
            strict_origin: true,
        })
    }

    #[test]
    fn empty_directory_returns_empty_projects() {
        let temp = TempDir::new().expect("tempdir");
        let canonical = temp.path().canonicalize().expect("canon");
        let state = build_state(canonical.clone());

        let out = run(&state);
        assert!(out.projects.is_empty());
        assert!(!out.git_available);
        assert_eq!(out.root, canonical.display().to_string());
    }

    #[test]
    fn registry_entries_appear_in_response() {
        let temp = TempDir::new().expect("tempdir");
        let canonical = temp.path().canonicalize().expect("canon");
        let state = build_state(canonical.clone());

        // Inject a fake entry directly into the registry to exercise the
        // mapping path independent of disk discovery.
        let abs = canonical.join("model.stmx");
        std::fs::write(&abs, "<root/>\n").expect("seed file");
        state.registry.upsert(
            abs.clone(),
            ProjectMeta {
                path: PathBuf::new(),
                format: ProjectFormat::Stmx,
                mtime: SystemTime::UNIX_EPOCH,
                size: 0,
                git: GitState::Untracked,
                version: 0,
                doc: Default::default(),
                last_disk_hash: 0,
                last_diagnostic_keys: std::collections::BTreeSet::new(),
            },
        );

        let out = run(&state);
        assert_eq!(out.projects.len(), 1);
        assert_eq!(out.projects[0].path, "model.stmx");
        assert_eq!(out.projects[0].format, "stmx");
        assert_eq!(out.projects[0].version, 0);
    }
}
