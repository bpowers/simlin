// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// `WatcherActor` owns the OS-level filesystem watch (via
// `notify-debouncer-full`'s recommended platform watcher) and feeds debounced
// batches into a tokio task that classifies and dispatches them. The
// debouncer runs its tick loop on a dedicated OS thread it spawns
// internally; we bridge it into tokio via an unbounded mpsc channel and a
// closure-based `DebounceEventHandler`. Note 1 in the phase plan called for
// the crate's `tokio` feature, but that feature only exists in 0.8+; on
// 0.7 we use the `FnMut(DebounceEventResult) + Send + 'static` blanket
// impl with a captured `tokio::sync::mpsc::UnboundedSender` to achieve the
// same wiring without the unstable RC.

//! Filesystem watcher actor for the Phase 4 disk -> Loro merge path.
//!
//! Architecture (per docs/design-plans/2026-04-05-server-rewrite.md):
//! a long-lived `WatcherActor` watches `state.root` recursively. The
//! debouncer coalesces bursts of events into 100ms batches; each batch
//! lands on the actor's tokio mpsc receiver. The actor's `run` loop
//! `tokio::select!`s over (a) `rx.recv()` for new batches and
//! (b) `shutdown.notified()` for graceful teardown.
//!
//! Per-event handlers (Subcomponent B): `handle_model_change` reads,
//! hash-compares for echo-suppression, parses, validates, and merges
//! through the registry's `merge_disk_change` primitive;
//! `handle_model_removal` drops the registry entry and emits
//! `WsMessage::ProjectRemoved`; `handle_git_change` invalidates the
//! `GitProbe` cache for the affected repo and re-evaluates each
//! registry entry inside it, broadcasting `ProjectChanged` for any
//! whose git status changed.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_full::DebouncedEvent;
use notify_debouncer_full::notify::event::{EventKind, ModifyKind};
use notify_debouncer_full::notify::{self, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::task::JoinHandle;

use crate::discovery::{classify_extension, is_excluded_dir};
use crate::events::{ChangeSource, WsMessage};
use crate::handlers::AppState;
use crate::hashing::content_hash;
use crate::parse::parse_to_datamodel;
use crate::registry::{GitState, ProjectFormat, ProjectMeta, RegistryError};
use crate::validation::{compute_baseline, validate_save_project};

/// Cross-task shutdown notifier. `main()` constructs one and passes clones
/// into long-lived actors (the watcher in Phase 4; the websocket loops can
/// adopt the same pattern in later phases). Calling `notify_waiters` on
/// the shared `Notify` wakes all `notified()` futures held inside
/// `tokio::select!` arms so each actor can exit its loop cleanly.
pub type ShutdownSignal = Arc<Notify>;

/// Errors raised while constructing or operating the watcher actor.
///
/// `Debouncer` carries the underlying `notify::Error` from the OS
/// watcher setup (most commonly: the root path doesn't exist or isn't a
/// directory, or the kernel's inotify/FSEvents/ReadDirectoryChangesW
/// resource limits are saturated).
#[derive(Debug)]
pub enum WatcherError {
    Debouncer(notify::Error),
}

impl std::fmt::Display for WatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatcherError::Debouncer(e) => write!(f, "filesystem watcher: {e}"),
        }
    }
}

impl std::error::Error for WatcherError {}

impl From<notify::Error> for WatcherError {
    fn from(value: notify::Error) -> Self {
        WatcherError::Debouncer(value)
    }
}

/// Whether a model file event represents a fresh creation or an update to
/// existing content. Renames are coalesced by the debouncer's file-id
/// cache to a single final-path event, so we don't need a separate
/// `Renamed` variant -- they show up as `Modified` against the destination
/// path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Modified,
}

/// One fully classified watcher event. `Ignored` is the dominant outcome
/// (most events are inside excluded dirs or for unrelated extensions);
/// `ModelFile` and `Removed` carry the format hint so the dispatch arms
/// can skip a re-lookup. `GitInternal` carries the repository root path
/// so the handler can scope its cache invalidation correctly when one
/// repo contains multiple model files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifiedEvent {
    /// A `.stmx`/`.xmile`/`.mdl`/`.sd.json` was created or updated.
    ModelFile {
        path: PathBuf,
        format: ProjectFormat,
        change: ChangeKind,
    },
    /// `.git/HEAD` or `.git/index` changed inside the named repository.
    GitInternal { repo_root: PathBuf },
    /// A model file was removed. `format` is `Some(...)` when the path's
    /// extension was recognizable; `None` for paths we wouldn't have
    /// dispatched a model event for in the first place (kept for
    /// completeness so callers can ignore them uniformly).
    Removed {
        path: PathBuf,
        format: Option<ProjectFormat>,
    },
    /// Anything else: events under excluded dirs, files with unknown
    /// extensions, metadata-only changes, etc.
    Ignored,
}

/// Classify a single debounced event. Operates on the *last* path in
/// `event.paths` -- after the debouncer's rename coalescing this is the
/// canonical "final" location of the affected file (for renames, the
/// destination; for everything else, the only path).
///
/// The `.git/HEAD` and `.git/index` special-cases run *before* the
/// universal-excluded-dir check. Without that ordering, a `.git/HEAD`
/// event would be dropped along with the rest of `.git/`. Per plan
/// note 8, those two paths are exactly the signals we want to
/// re-trigger git-status recomputation on.
///
/// macOS rename quirk (plan note 3): FSEvents does not always emit
/// paired rename events; a `Modify(Name(Any))` for the destination
/// path can show up alone. We classify those as `Modified` so the
/// merge layer re-reads the file and absorbs the new content -- the
/// path-id cache has already given us the canonical destination.
pub fn classify(event: &DebouncedEvent) -> ClassifiedEvent {
    // The debouncer puts rename pairs in [from, to] order; the
    // destination ("to") is the last path. For single-path events the
    // last path is the only path. Either way, classifying on the
    // last path is what we want.
    let path = match event.paths.last() {
        Some(p) => p.clone(),
        None => return ClassifiedEvent::Ignored,
    };

    if let Some(repo_root) = git_internal_repo_root(&path) {
        return ClassifiedEvent::GitInternal { repo_root };
    }

    if path_traverses_excluded_dir(&path) {
        return ClassifiedEvent::Ignored;
    }

    let Some(format) = classify_extension(&path) else {
        return ClassifiedEvent::Ignored;
    };

    match event.kind {
        EventKind::Create(_) => ClassifiedEvent::ModelFile {
            path,
            format,
            change: ChangeKind::Created,
        },
        EventKind::Modify(ModifyKind::Name(_)) => {
            // macOS / FSEvents path: rename-without-content-change.
            // Treat as Modified so the merge layer re-reads the file.
            ClassifiedEvent::ModelFile {
                path,
                format,
                change: ChangeKind::Modified,
            }
        }
        EventKind::Modify(_) => ClassifiedEvent::ModelFile {
            path,
            format,
            change: ChangeKind::Modified,
        },
        EventKind::Remove(_) => ClassifiedEvent::Removed {
            path,
            format: Some(format),
        },
        // Access events, metadata-only changes outside of `Modify`,
        // and the catch-all `Any` aren't actionable for our merge
        // path. The debouncer doesn't emit `Any` for known platform
        // events, so this arm is mostly defense-in-depth.
        _ => ClassifiedEvent::Ignored,
    }
}

/// Returns `Some(repo_root)` when `path` looks like `.../<repo>/.git/HEAD`
/// or `.../<repo>/.git/index`. The repo root is the path immediately
/// before the `.git` segment. `None` for paths that aren't one of those
/// two specific files.
fn git_internal_repo_root(path: &Path) -> Option<PathBuf> {
    let components: Vec<Component> = path.components().collect();
    // Need at least: <something> / .git / HEAD-or-index
    if components.len() < 2 {
        return None;
    }
    let last = components.last()?.as_os_str();
    let is_head_or_index = last == "HEAD" || last == "index";
    if !is_head_or_index {
        return None;
    }
    // The `.git` segment must be exactly one component before the
    // last; deeper paths like `.git/refs/heads/main` are not what we
    // want to fire git-status invalidation for (they'd be too noisy
    // and the index/HEAD pair is the canonical signal).
    let parent = components.get(components.len() - 2)?.as_os_str();
    if parent != ".git" {
        return None;
    }
    // Walk back one more to find the repo root. If `.git` is at the
    // very top of the path (`./.git/HEAD` with no preceding
    // component), the repo root is the empty path -- which is fine,
    // the handler interprets it as "the watcher root itself".
    let mut repo_root = PathBuf::new();
    for component in &components[..components.len() - 2] {
        repo_root.push(component.as_os_str());
    }
    Some(repo_root)
}

/// True if any *normal* component of `path` matches `is_excluded_dir`.
/// `.git` triggers the universal exclusion list, but we've already
/// special-cased `.git/HEAD` and `.git/index` upstream.
fn path_traverses_excluded_dir(path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(name) => name.to_str().map(is_excluded_dir).unwrap_or(false),
        _ => false,
    })
}

/// For `path = "/dir/foo.mdl"`, return `/dir/foo.sd.json`. Same rule
/// the GET handler and writer use; kept here to avoid a cross-module
/// dep just for the sidecar-name predicate.
fn sidecar_for_mdl(mdl_path: &Path) -> PathBuf {
    let parent = mdl_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = mdl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    parent.join(format!("{stem}.sd.json"))
}

/// Render a relative `Path` as a string with forward-slash separators.
/// Mirrors handlers.rs's local helper -- we duplicate here rather than
/// expose because the watcher is a sibling module and the converter is
/// trivially small.
fn path_to_forward_slash(path: &Path) -> String {
    let display = path.to_string_lossy().into_owned();
    if MAIN_SEPARATOR == '/' {
        display
    } else {
        display.replace(MAIN_SEPARATOR, "/")
    }
}

/// Long-lived actor that bridges the OS filesystem watcher into tokio.
pub struct WatcherActor {
    #[allow(dead_code)]
    state: AppState,
    rx: UnboundedReceiver<DebounceEventResult>,
    /// Owned shutdown future captured *synchronously* in `spawn_watcher`
    /// before the actor task starts running. This closes a race that
    /// matters in tests (and could matter in production on a
    /// fast-shutdown path): if `notify_waiters()` runs between the
    /// spawn and the actor's first poll, a future created lazily inside
    /// the loop would miss it. `Notify::notified_owned` captures the
    /// `num_notify_waiters_calls` counter at construction time, so a
    /// notification that arrives before the first poll still wakes us.
    shutdown: tokio::sync::futures::OwnedNotified,
    /// Hold the `Debouncer` so its OS-level watch and tick thread stay
    /// alive for the actor's lifetime. Dropping the actor drops the
    /// debouncer, which signals its background thread to stop and
    /// releases the kernel-level watch.
    #[allow(dead_code)]
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl WatcherActor {
    /// Drive the actor until the shutdown signal fires or the channel
    /// closes (which would indicate the debouncer crashed).
    ///
    /// The shutdown `OwnedNotified` future was captured synchronously
    /// in `spawn_watcher` (before this task started running); pinning
    /// it here just lets us poll it via `select!`. The capture-time
    /// counter on `OwnedNotified` is what makes
    /// `notify_waiters()`-then-`spawn_watcher` and
    /// `spawn_watcher`-then-`notify_waiters` both correct.
    async fn run(self) {
        let WatcherActor {
            state,
            mut rx,
            shutdown,
            debouncer: _debouncer,
        } = self;
        let mut shutdown = Box::pin(shutdown);
        loop {
            tokio::select! {
                Some(result) = rx.recv() => {
                    Self::handle_batch(&state, result).await;
                }
                _ = &mut shutdown => {
                    tracing::debug!("watcher actor: shutdown signal received");
                    break;
                }
            }
        }
        drop(_debouncer);
    }

    /// Classify each event in the batch and dispatch to the appropriate
    /// handler. The handlers themselves are `tracing::debug!` stubs in
    /// Subcomponent A; Subcomponent B (Tasks 5/6/7) replaces them with
    /// real read/parse/validate/merge logic.
    ///
    /// Errors from the debouncer are logged but never propagated -- the
    /// actor keeps running so a transient watch failure (rare) doesn't
    /// take down the server.
    ///
    /// Takes `&AppState` rather than `&self` because `run` already
    /// destructured `self` into individual fields to keep the partial
    /// borrow checker happy across the `select!` arms.
    async fn handle_batch(state: &AppState, result: DebounceEventResult) {
        let events = match result {
            Ok(events) => events,
            Err(errors) => {
                for err in errors {
                    tracing::warn!(error = %err, "watcher actor: debouncer error");
                }
                return;
            }
        };

        for event in &events {
            match classify(event) {
                ClassifiedEvent::ModelFile {
                    path,
                    format,
                    change,
                } => {
                    Self::handle_model_change(state, path, format, change).await;
                }
                ClassifiedEvent::Removed { path, format } => {
                    Self::handle_model_removal(state, path, format).await;
                }
                ClassifiedEvent::GitInternal { repo_root } => {
                    Self::handle_git_change(state, repo_root).await;
                }
                ClassifiedEvent::Ignored => {
                    // Most events are ignored; tracing at debug keeps
                    // logs quiet under normal operation but lets us
                    // diagnose missing-event reports by raising the
                    // log level.
                    tracing::debug!(
                        kind = ?event.kind,
                        paths = ?event.paths,
                        "watcher actor: ignored event"
                    );
                }
            }
        }
    }

    /// Disk-side merge path. Reads the file, short-circuits on byte
    /// echo (matching the cached `last_disk_hash` from our own atomic
    /// writes), parses + validates against the current Loro tip's
    /// baseline, and applies the new state through
    /// `registry.merge_disk_change`. Failures (read, parse, validation)
    /// preserve the last-known-good in-memory state and only emit a
    /// `tracing::warn!` so the operator can see something went wrong.
    ///
    /// On success, broadcasts `ProjectChanged { source: Disk }` so the
    /// browser remounts the editor with the merged state.
    ///
    /// Sidecar override (plan note 9): if `format == Mdl` and a sibling
    /// `.sd.json` exists, ignore the `.mdl` event entirely. The sidecar
    /// is canonical once it exists; an event on the `.mdl` is stale.
    async fn handle_model_change(
        state: &AppState,
        path: PathBuf,
        format: ProjectFormat,
        change: ChangeKind,
    ) {
        if format == ProjectFormat::Mdl {
            let sidecar = sidecar_for_mdl(&path);
            if sidecar.is_file() {
                tracing::debug!(
                    path = %path.display(),
                    sidecar = %sidecar.display(),
                    "watcher: ignoring .mdl event because sidecar exists"
                );
                return;
            }
        }

        // Canonicalize before reading so the bytes come from the path the
        // registry is keyed by. When `path` and its canonical form differ
        // (e.g. a symlink inside the watched tree), reading `path` would hash
        // bytes that may not match the content the registry entry was seeded
        // with, producing a spurious echo-suppression miss.
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(err) => {
                tracing::debug!(
                    path = %path.display(),
                    error = %err,
                    "watcher: canonicalize failed; skipping merge"
                );
                return;
            }
        };

        let bytes = match tokio::fs::read(&canonical).await {
            Ok(b) => b,
            Err(err) => {
                // The file may have been removed between the event and
                // the read; that race is a Removed event in disguise
                // and the dispatch routes it elsewhere. Either way,
                // there's nothing to merge here.
                tracing::debug!(
                    path = %canonical.display(),
                    error = %err,
                    "watcher: read failed (likely vanished); skipping merge"
                );
                return;
            }
        };

        let new_hash = content_hash(&bytes);

        // Echo suppression: if the bytes hash to the same value the save
        // handler stored after its own atomic_write, we're seeing our own
        // event come back through the OS watcher. Skip the merge.
        if let Some(meta) = state.registry.get(&canonical) {
            if meta.last_disk_hash == new_hash {
                tracing::debug!(
                    path = %canonical.display(),
                    hash = %format!("{new_hash:#x}"),
                    "watcher: echo-suppressed (hash matches last save)"
                );
                return;
            }
        } else if change != ChangeKind::Created {
            // Create-on-an-unknown-path is the legitimate "freshly
            // dropped a model in the watched dir" case; for Modify/Other
            // events on paths we don't track we just skip.
            tracing::debug!(
                path = %canonical.display(),
                "watcher: modify on path not in registry; ignoring"
            );
            return;
        }

        let contents = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    path = %canonical.display(),
                    error = %err,
                    "watcher: file is not valid UTF-8; ignoring change"
                );
                return;
            }
        };

        let new_project = match parse_to_datamodel(&canonical, format, contents) {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(
                    path = %canonical.display(),
                    error = %err,
                    "watcher: parse failed; preserving last-known-good in-memory state"
                );
                return;
            }
        };

        // Compute the baseline against the current Loro tip so an external
        // edit that fixes pre-existing errors is accepted. If the doc
        // hasn't been hydrated yet (e.g. a Created event on a fresh path),
        // the baseline is empty, which means any new error will block the
        // merge — appropriate for a brand-new file we're about to track.
        let baseline = match state.registry.get_or_init_doc(&canonical) {
            Ok(doc) => match doc.export_canonical_json() {
                Ok(value) => match serde_json::from_value::<simlin_engine::json::Project>(value) {
                    Ok(json_project) => {
                        let project: simlin_engine::datamodel::Project = json_project.into();
                        compute_baseline(&project)
                    }
                    Err(err) => {
                        tracing::warn!(
                            path = %canonical.display(),
                            error = %err,
                            "watcher: doc state is not a project; using empty baseline"
                        );
                        Default::default()
                    }
                },
                Err(err) => {
                    tracing::warn!(
                        path = %canonical.display(),
                        error = %err,
                        "watcher: doc export failed; using empty baseline"
                    );
                    Default::default()
                }
            },
            Err(_) => {
                // No doc yet for this entry (the registry doesn't even
                // know about it). Fall through with an empty baseline;
                // the subsequent ensure_or_get below seeds the entry.
                Default::default()
            }
        };

        let outcome = validate_save_project(&new_project, &baseline);
        if !outcome.new_errors.is_empty() {
            tracing::warn!(
                path = %canonical.display(),
                errors = ?outcome.new_errors,
                "watcher: validation failed; preserving last-known-good"
            );
            return;
        }

        // Convert the validated project to canonical JSON for the merge.
        // This goes through json::Project so the doc always sees the
        // canonical engine shape regardless of subtle formatting in the
        // file (case, whitespace, optional-field omission, etc.).
        let canonical_project: simlin_engine::json::Project = (&new_project).into();
        let canonical_value = match serde_json::to_value(&canonical_project) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    path = %canonical.display(),
                    error = %err,
                    "watcher: serialize canonical json failed"
                );
                return;
            }
        };

        // For a brand-new path the registry has nothing to merge against;
        // ensure a default ProjectMeta exists (with `last_disk_hash = 0`
        // so we don't echo-suppress the very next legitimate edit).
        if state.registry.get(&canonical).is_none() {
            let metadata = std::fs::metadata(&canonical);
            let (mtime, size) = match metadata {
                Ok(m) => (
                    m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                    m.len(),
                ),
                Err(_) => (std::time::SystemTime::UNIX_EPOCH, 0),
            };
            state
                .registry
                .ensure_or_get(canonical.clone(), || ProjectMeta {
                    path: PathBuf::new(),
                    format,
                    mtime,
                    size,
                    git: GitState::Untracked,
                    version: 0,
                    doc: Default::default(),
                    last_disk_hash: 0,
                    last_diagnostic_keys: std::collections::BTreeSet::new(),
                });
        }

        let new_version = match state
            .registry
            .merge_disk_change(&canonical, &canonical_value)
        {
            Ok(v) => v,
            Err(RegistryError::NotFound) => {
                tracing::warn!(
                    path = %canonical.display(),
                    "watcher: registry entry vanished between ensure_or_get and merge"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(
                    path = %canonical.display(),
                    error = %err,
                    "watcher: merge failed; preserving last-known-good"
                );
                return;
            }
        };

        // Refresh the meta + last_disk_hash so a subsequent identical
        // event (e.g., the editor wrote the same bytes twice) is
        // echo-suppressed. The hash is computed from the bytes WE just
        // ingested, not from a re-read; that closes a TOCTOU window
        // where another writer comes in between the merge and the
        // metadata refresh.
        if let Ok(metadata) = std::fs::metadata(&canonical)
            && let Ok(mtime) = metadata.modified()
        {
            state
                .registry
                .refresh_after_write(&canonical, mtime, metadata.len(), new_hash);
        }

        let display_path = match canonical.strip_prefix(state.root.as_ref()) {
            Ok(rel) => path_to_forward_slash(rel),
            Err(_) => path_to_forward_slash(&canonical),
        };

        tracing::info!(
            path = %display_path,
            version = new_version,
            "watcher: merged disk change"
        );

        state.events.publish(WsMessage::ProjectChanged {
            path: display_path,
            version: new_version,
            source: ChangeSource::Disk,
        });
    }

    /// Drop the registry entry for `path` and broadcast `ProjectRemoved`
    /// so the SPA's sidebar can drop the entry. The path is not
    /// canonicalized here because canonicalize() requires the file to
    /// exist; we use the post-strip-prefix relative path that matches
    /// the registry's display key.
    ///
    /// On Linux, `atomic_write`'s rename generates a `Remove(Any)` event
    /// for the destination path because the old inode is unlinked, even
    /// though the file immediately reappears with new content (via
    /// `IN_MOVED_TO`). We suppress the removal in that case by checking
    /// whether the file still exists before touching the registry: if it
    /// does, the next `Create(File)` event in the same batch will drive
    /// the merge correctly.
    ///
    /// Sidecar pairing on .mdl: when the .sd.json sidecar is removed the
    /// .mdl theoretically becomes the source-of-truth again. Implementing
    /// that round-trip is documented in the design plan (note 9) and
    /// deferred to Phase 8 polish; for Phase 4 we simply remove whichever
    /// path the watcher tells us about. The registry is permissive
    /// (`remove` is a no-op for unknown keys) so this is safe even when
    /// the path was already absent.
    async fn handle_model_removal(state: &AppState, path: PathBuf, format: Option<ProjectFormat>) {
        // If the file still exists on disk, the Remove event came from an
        // atomic rename-over (the old inode was unlinked but the path
        // immediately reappeared with new content). The subsequent Create
        // event in this same batch will drive the merge; we must not drop
        // the registry entry here or echo-suppression loses its state.
        if path.exists() {
            tracing::debug!(
                path = %path.display(),
                "watcher: Remove event for path that still exists; skipping (atomic-write rename)"
            );
            return;
        }

        // The registry key is the canonical path; canonicalize() can't
        // run on a now-deleted file, so we strip the watcher root prefix
        // and recompose against `state.root` (which is canonicalized at
        // server startup). For paths outside the root we fall back to
        // the original `path`; the registry's `remove` is no-op-on-miss.
        let registry_key = match path.strip_prefix(state.root.as_ref()) {
            Ok(rel) => state.root.as_ref().join(rel),
            Err(_) => path.clone(),
        };

        // Was the entry actually present? `remove` doesn't tell us, so
        // we look up first; this lets us suppress spurious
        // `ProjectRemoved` broadcasts for paths the registry never
        // tracked (e.g., a file deleted under the watched root that
        // matched our extension filter but was never discovered).
        let was_present = state.registry.get(&registry_key).is_some();
        state.registry.remove(&registry_key);

        if !was_present {
            tracing::debug!(
                path = %path.display(),
                ?format,
                "watcher: removal for path not in registry; skipping broadcast"
            );
            return;
        }

        let display_path = match path.strip_prefix(state.root.as_ref()) {
            Ok(rel) => path_to_forward_slash(rel),
            Err(_) => path_to_forward_slash(&path),
        };

        tracing::info!(
            path = %display_path,
            ?format,
            "watcher: model file removed"
        );

        state
            .events
            .publish(WsMessage::ProjectRemoved { path: display_path });
    }

    /// React to a `.git/HEAD`/`.git/index` mutation by invalidating
    /// the `GitProbe`'s per-repo cache for `repo_root`, then re-running
    /// `status_for` on every registry entry inside the repo. Each
    /// entry whose git state actually changed is updated in place and
    /// gets a `ProjectChanged { source: Disk }` broadcast (the model
    /// itself didn't change, but its sidebar indicator did, and the
    /// frontend uses the same broadcast channel to refresh).
    ///
    /// Per plan note 8 the watcher only fires for HEAD/index, never
    /// for `.git/objects/...` or `.git/refs/...`; if a future change
    /// adds more triggers we'll just process them with the same
    /// "invalidate + re-status" logic.
    ///
    /// Recomputation is scoped to known registry entries. Fresh files that
    /// exist on disk inside the affected repo but are not yet in the registry
    /// get no update here; they come in via `handle_model_change` when their
    /// content changes. A `.git/HEAD` event alone does not drive discovery
    /// of new files.
    async fn handle_git_change(state: &AppState, repo_root: PathBuf) {
        // Resolve `repo_root` against `state.root` if it came in
        // relative (the classifier emits the path-as-given, so a
        // top-level `.git` at the watcher root produces an empty
        // PathBuf). For inner repos we still get an absolute path.
        let canonical_repo = if repo_root.as_os_str().is_empty() {
            state.root.as_ref().clone()
        } else if repo_root.is_absolute() {
            repo_root.clone()
        } else {
            state.root.as_ref().join(&repo_root)
        };

        state.git.invalidate_repo_cache(&canonical_repo);

        let snapshot = state.registry.snapshot();
        for entry in snapshot {
            // The snapshot's path is relative-to-root; rebuild the
            // absolute key the same way the registry does internally.
            let abs = state.root.as_ref().join(&entry.path);
            // Skip files outside the affected repo. enclosing_git_root
            // walks upward until it finds a `.git`; if that root
            // doesn't match the affected repo, this entry's git
            // status didn't change.
            let entry_repo = match crate::git::enclosing_git_root(&abs) {
                Some(r) => r,
                None => continue,
            };
            if entry_repo != canonical_repo {
                continue;
            }
            let new_git = state.git.status_for(&abs);
            if new_git == entry.git {
                continue;
            }
            // Update the registry entry's git field in place. We
            // rebuild a fresh ProjectMeta because the registry's
            // upsert overwrites the whole entry; preserve every
            // non-git field.
            let updated = ProjectMeta {
                path: PathBuf::new(),
                format: entry.format,
                mtime: entry.mtime,
                size: entry.size,
                git: new_git,
                version: entry.version,
                doc: entry.doc.clone(),
                last_disk_hash: entry.last_disk_hash,
                last_diagnostic_keys: entry.last_diagnostic_keys.clone(),
            };
            state.registry.upsert_preserve_version(abs.clone(), updated);

            let display_path = match abs.strip_prefix(state.root.as_ref()) {
                Ok(rel) => path_to_forward_slash(rel),
                Err(_) => path_to_forward_slash(&abs),
            };
            tracing::debug!(
                path = %display_path,
                version = entry.version,
                "watcher: git status changed; broadcasting ProjectChanged"
            );
            state.events.publish(WsMessage::ProjectChanged {
                path: display_path,
                version: entry.version,
                source: ChangeSource::Disk,
            });
        }
    }
}

/// Construct and spawn the watcher actor.
///
/// Returns the join handle for the spawned tokio task. The caller must
/// hold onto it (for graceful shutdown / wait-on-exit) — dropping the
/// handle does not abort the task. The debouncer is moved into the actor
/// and dropped when the task exits, releasing the OS-level watch.
pub fn spawn_watcher(
    state: AppState,
    shutdown: ShutdownSignal,
) -> Result<JoinHandle<()>, WatcherError> {
    let (tx, rx) = unbounded_channel::<DebounceEventResult>();

    // Bridge from the debouncer's `DebounceEventHandler` (called on the
    // debouncer's OS thread) into tokio via an unbounded mpsc. The
    // closure satisfies the `FnMut + Send + 'static` blanket impl on
    // `DebounceEventHandler`. `send` drops events if the receiver is
    // gone (i.e. the actor exited); that's the desired behavior.
    let bridge = move |result: DebounceEventResult| {
        let _ = tx.send(result);
    };

    // 100ms debounce window -- plan note 1. tick_rate=None means the
    // debouncer picks 1/4 of the timeout (25ms) automatically.
    let mut debouncer = new_debouncer(Duration::from_millis(100), None, bridge)?;

    // Watch the root recursively (plan note 2: file-level watches die
    // after atomic_write's rename, so we anchor on the parent directory
    // and let the debouncer's file-id cache normalize the final path).
    let root: &Path = state.root.as_ref();
    debouncer.watch(root, RecursiveMode::Recursive)?;

    // Capture the shutdown future synchronously so the
    // `num_notify_waiters_calls` snapshot is taken *before* the spawned
    // task ever runs. This matters when a caller does
    // `spawn_watcher(...); shutdown.notify_waiters();` in close
    // succession (notably in tests): with a lazy-construction approach
    // the actor would register its waiter after the notification was
    // already counted, and the notification would be silently lost.
    let shutdown_future = shutdown.notified_owned();

    let actor = WatcherActor {
        state,
        rx,
        shutdown: shutdown_future,
        debouncer,
    };

    let handle = tokio::spawn(actor.run());
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: build a minimal AppState rooted at `dir`.
    fn build_app_state(dir: &Path) -> AppState {
        use crate::events::EventBus;
        use crate::git::GitProbe;
        use crate::registry::ProjectRegistry;

        let canonical = dir.canonicalize().expect("canonicalize");
        AppState {
            registry: Arc::new(ProjectRegistry::new(canonical.clone())),
            git: Arc::new(GitProbe::unavailable_for_tests()),
            root: Arc::new(canonical),
            events: Arc::new(EventBus::new()),
            launch_token: Arc::new("test-token".to_string()),
        }
    }

    #[tokio::test]
    async fn spawn_watcher_returns_join_handle_and_terminates_on_shutdown() {
        // Construct a watcher rooted at a tempdir; verify the spawn returns
        // a join handle, the shutdown signal terminates the actor cleanly,
        // and the join handle resolves promptly.
        let dir = TempDir::new().expect("tempdir");
        let state = build_app_state(dir.path());
        let shutdown: ShutdownSignal = Arc::new(Notify::new());

        let handle = spawn_watcher(state, shutdown.clone()).expect("watcher spawns successfully");

        shutdown.notify_waiters();

        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("actor exited within 500ms")
            .expect("actor task did not panic");
    }

    #[test]
    fn watcher_error_displays_underlying_notify_message() {
        // Smoke check: WatcherError -> Display delegates to the inner
        // notify::Error so server logs show actionable detail.
        let inner = notify::Error::generic("boom");
        let err = WatcherError::from(inner);
        let msg = format!("{err}");
        assert!(msg.contains("filesystem watcher"));
        assert!(msg.contains("boom"));
    }

    use notify_debouncer_full::notify::Event;
    use notify_debouncer_full::notify::event::{
        CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode,
    };
    use std::time::Instant;

    fn make_debounced(kind: EventKind, paths: Vec<PathBuf>) -> DebouncedEvent {
        let mut event = Event::new(kind);
        event.paths = paths;
        DebouncedEvent::new(event, Instant::now())
    }

    #[test]
    fn classify_modify_under_excluded_dir_is_ignored() {
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![PathBuf::from("/repo/node_modules/foo.stmx")],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[test]
    fn classify_target_under_excluded_dir_is_ignored() {
        // `target` is on the universal denylist. Any model file under
        // it should be filtered out -- e.g. a build script that
        // generates `.stmx` files into `target/` should not trigger
        // any merges.
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![PathBuf::from("/repo/target/build/out.stmx")],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[test]
    fn classify_modify_on_stmx_returns_model_file_modified() {
        let path = PathBuf::from("/repo/models/x.stmx");
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![path.clone()],
        );
        assert_eq!(
            classify(&event),
            ClassifiedEvent::ModelFile {
                path,
                format: ProjectFormat::Stmx,
                change: ChangeKind::Modified,
            }
        );
    }

    #[test]
    fn classify_create_on_stmx_returns_model_file_created() {
        let path = PathBuf::from("/repo/models/x.stmx");
        let event = make_debounced(EventKind::Create(CreateKind::File), vec![path.clone()]);
        assert_eq!(
            classify(&event),
            ClassifiedEvent::ModelFile {
                path,
                format: ProjectFormat::Stmx,
                change: ChangeKind::Created,
            }
        );
    }

    #[test]
    fn classify_remove_on_stmx_returns_removed_with_format() {
        let path = PathBuf::from("/repo/models/x.stmx");
        let event = make_debounced(EventKind::Remove(RemoveKind::File), vec![path.clone()]);
        assert_eq!(
            classify(&event),
            ClassifiedEvent::Removed {
                path,
                format: Some(ProjectFormat::Stmx),
            }
        );
    }

    #[test]
    fn classify_modify_name_on_stmx_treated_as_modified_for_macos_quirk() {
        // Per plan note 3: macOS FSEvents may emit a `Modify(Name(Any))`
        // for renames without a matching content event. We classify
        // those as Modified so the merge layer re-reads and ingests
        // the new content keyed at the destination path.
        let path = PathBuf::from("/repo/models/x.stmx");
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Name(RenameMode::Any)),
            vec![path.clone()],
        );
        assert_eq!(
            classify(&event),
            ClassifiedEvent::ModelFile {
                path,
                format: ProjectFormat::Stmx,
                change: ChangeKind::Modified,
            }
        );
    }

    #[test]
    fn classify_unknown_extension_is_ignored() {
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![PathBuf::from("/repo/models/notes.md")],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[test]
    fn classify_git_head_returns_git_internal_with_repo_root() {
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![PathBuf::from("/work/repo/.git/HEAD")],
        );
        assert_eq!(
            classify(&event),
            ClassifiedEvent::GitInternal {
                repo_root: PathBuf::from("/work/repo"),
            }
        );
    }

    #[test]
    fn classify_git_index_returns_git_internal_with_repo_root() {
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![PathBuf::from("/work/repo/.git/index")],
        );
        assert_eq!(
            classify(&event),
            ClassifiedEvent::GitInternal {
                repo_root: PathBuf::from("/work/repo"),
            }
        );
    }

    #[test]
    fn classify_git_objects_path_is_ignored() {
        // Only HEAD and index trigger GitInternal; other paths under
        // `.git/` are too noisy to act on (object writes, refs/, etc).
        // The discovery exclusion handles them.
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![PathBuf::from("/work/repo/.git/objects/abc")],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[test]
    fn classify_git_packed_refs_is_ignored() {
        // Verifies that the .git universal-exclusion still catches
        // non-HEAD/index files inside .git/.
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![PathBuf::from("/work/repo/.git/refs/heads/main")],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[test]
    fn classify_event_with_no_paths_is_ignored() {
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            vec![],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[test]
    fn classify_uses_last_path_for_rename_pair() {
        // For renames the debouncer emits paths = [from, to]; the
        // classifier should key off `to` (the new canonical location).
        // Here `from` was a `.stmx` and `to` is a `.md` -- so the
        // event should be Ignored (the file is no longer a model).
        let from = PathBuf::from("/repo/models/x.stmx");
        let to = PathBuf::from("/repo/notes/x.md");
        let event = make_debounced(
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
            vec![from, to],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[test]
    fn classify_remove_on_unknown_extension_is_ignored() {
        // Removing a non-model file shouldn't fire a Removed event;
        // we only care about model-file removals for the registry's
        // drop path.
        let event = make_debounced(
            EventKind::Remove(RemoveKind::File),
            vec![PathBuf::from("/repo/notes.md")],
        );
        assert_eq!(classify(&event), ClassifiedEvent::Ignored);
    }

    #[tokio::test]
    async fn handle_batch_dispatches_each_classified_variant_without_panic() {
        // The handlers are stubs in Subcomponent A; this test verifies
        // that the dispatch wiring runs to completion for each variant
        // (no panics, no awaiters left dangling). When Subcomponent B
        // replaces the stubs with real handlers, this test will need to
        // be expanded to assert their observable side effects -- but
        // the dispatch shape itself stays the same.
        let dir = TempDir::new().expect("tempdir");
        let state = build_app_state(dir.path());

        let events = vec![
            // ModelFile / Modified
            make_debounced(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                vec![PathBuf::from("/repo/models/x.stmx")],
            ),
            // ModelFile / Created
            make_debounced(
                EventKind::Create(CreateKind::File),
                vec![PathBuf::from("/repo/models/y.stmx")],
            ),
            // Removed
            make_debounced(
                EventKind::Remove(RemoveKind::File),
                vec![PathBuf::from("/repo/models/z.stmx")],
            ),
            // GitInternal
            make_debounced(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                vec![PathBuf::from("/repo/.git/HEAD")],
            ),
            // Ignored
            make_debounced(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                vec![PathBuf::from("/repo/notes.md")],
            ),
        ];
        let result: DebounceEventResult = Ok(events);
        WatcherActor::handle_batch(&state, result).await;
    }

    #[tokio::test]
    async fn handle_batch_logs_errors_without_propagating() {
        // An Err arm from the debouncer (rare in practice -- most
        // commonly a transient inotify resource hiccup) should be
        // logged but not crash the actor. We only assert the call
        // returns without panic; tracing output is not captured here.
        let dir = TempDir::new().expect("tempdir");
        let state = build_app_state(dir.path());
        let result: DebounceEventResult = Err(vec![notify::Error::generic("simulated failure")]);
        WatcherActor::handle_batch(&state, result).await;
    }
}
