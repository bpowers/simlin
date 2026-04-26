// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! In-memory registry of discovered system-dynamics projects.
//!
//! The registry is the single source of truth for "what files does the server
//! know about right now". Discovery + git probing populate it; HTTP handlers
//! read from it. Path keys are absolute and canonicalized so callers can look
//! up by `fs::canonicalize` results without worrying about symlinks or `./`
//! prefixes; the `path` field stored on each `ProjectMeta` is the
//! relative-to-root display form because that's what the SPA renders.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use serde::Serialize;

use crate::loro_doc::{MergeError, ProjectDoc};
use crate::parse::{ParseError, parse_to_datamodel};

/// Source-format hint for a discovered project. Lowercased on the wire so the
/// SPA can switch on string discriminants without re-encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFormat {
    Stmx,
    Xmile,
    Mdl,
    SdJson,
}

impl std::fmt::Display for ProjectFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProjectFormat::Stmx => "stmx",
            ProjectFormat::Xmile => "xmile",
            ProjectFormat::Mdl => "mdl",
            ProjectFormat::SdJson => "sd_json",
        };
        f.write_str(s)
    }
}

/// Per-file VCS state. `Unavailable` means git itself is missing from PATH;
/// `Untracked` means we found no enclosing repository (or the file lives
/// inside a working tree but isn't in the index, e.g. an ignored extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GitState {
    Tracked { dirty: bool },
    Untracked,
    Unavailable,
}

/// Snapshot of a single discovered project. `path` is *relative to the
/// registry root* with platform-native separators; the registry's
/// absolute-path key is the canonical lookup form.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectMeta {
    pub path: PathBuf,
    pub format: ProjectFormat,
    pub mtime: SystemTime,
    pub size: u64,
    pub git: GitState,
    /// Optimistic-lock counter. Phase 1 always reports `0`; Phase 2 will
    /// increment on each successful save so the SPA can detect concurrent
    /// modification.
    pub version: u64,
    /// Lazily-hydrated in-memory CRDT document for this project.
    /// `None` until the first read or write; populated by
    /// `ProjectRegistry::get_or_init_doc`. Skipped in `Serialize`
    /// (the SPA never sees it on the wire) and lives behind an
    /// `Arc<RwLock<Option<...>>>` so cloned `ProjectMeta` instances
    /// share the same hydration slot — initializing through one clone
    /// is observable through every other.
    #[serde(skip)]
    pub doc: Arc<RwLock<Option<Arc<ProjectDoc>>>>,
    /// XXH3-64 hash of the bytes most recently written to this path by the
    /// server's save handler. Used by the Phase 4 file watcher for echo
    /// suppression: when the watcher sees a `Modify` event for the same
    /// path, it computes the hash of the current on-disk bytes and skips
    /// re-merging when the hash matches this value (i.e. we're seeing our
    /// own atomic-write, not an external edit).
    ///
    /// Defaults to `0` and stays at `0` for entries the server has never
    /// written. A genuine on-disk hash that happens to be `0` would also
    /// short-circuit, but the false-positive rate is `2^-64` per write of
    /// arbitrary content; over millions of writes the cumulative rate
    /// remains negligible. A false positive causes redundant work, not a
    /// correctness violation.
    #[serde(skip)]
    pub last_disk_hash: u64,
    /// Canonical key set of the project's most recently computed
    /// diagnostics — pairs of `(error_code, variable_name)`. Used by
    /// the save / watcher / MCP merge paths to decide whether the
    /// post-merge diagnostic set actually changed; only differing sets
    /// produce a `WsMessage::DiagnosticsChanged` broadcast.
    ///
    /// Empty on a freshly inserted entry. The first merge after insert
    /// compares against this empty set so any errors present in the
    /// new state surface as `DiagnosticsChanged` (and a clean project
    /// stays silent on its first save).
    #[serde(skip)]
    pub last_diagnostic_keys: BTreeSet<(String, Option<String>)>,
}

/// Failures produced by registry operations that need to report a
/// distinguishable error rather than just succeed-or-do-nothing.
#[derive(Debug)]
pub enum RegistryError {
    /// No entry exists for the given absolute path.
    NotFound,
    /// An entry already exists at the target path. Returned by
    /// `rename_entry` when `to` is already tracked, so the caller can
    /// decide whether to displace the existing entry explicitly (e.g. by
    /// publishing `ProjectRemoved` for `to` first) rather than silently
    /// overwriting it.
    AlreadyExists,
    /// The caller's `expected_version` did not match the entry's stored
    /// version. `actual` is the current value as observed under the lock so
    /// the caller can refetch against it.
    VersionMismatch { expected: u64, actual: u64 },
    /// Lazy `ProjectDoc` hydration failed (file disappeared between
    /// discovery and first access, parse failure, etc.). Carries the
    /// underlying cause so callers can surface a useful error to the
    /// client. Phase 4's file watcher will further mitigate the
    /// "file disappeared" race by keeping the registry's view of
    /// the filesystem fresh.
    HydrationFailed(String),
}

impl PartialEq for RegistryError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (RegistryError::NotFound, RegistryError::NotFound) => true,
            (RegistryError::AlreadyExists, RegistryError::AlreadyExists) => true,
            (
                RegistryError::VersionMismatch {
                    expected: a,
                    actual: b,
                },
                RegistryError::VersionMismatch {
                    expected: c,
                    actual: d,
                },
            ) => a == c && b == d,
            (RegistryError::HydrationFailed(a), RegistryError::HydrationFailed(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for RegistryError {}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NotFound => write!(f, "registry entry not found"),
            RegistryError::AlreadyExists => write!(f, "registry entry already exists at target"),
            RegistryError::VersionMismatch { expected, actual } => {
                write!(f, "version mismatch: expected {expected}, actual {actual}")
            }
            RegistryError::HydrationFailed(msg) => write!(f, "doc hydration failed: {msg}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Concurrent registry of `ProjectMeta` keyed by absolute canonical path.
///
/// Cloning is cheap (`Arc`-shared inner state). The internal `RwLock` uses
/// `std`'s implementation; `parking_lot` would be marginally faster but isn't
/// in the workspace today and dragging it in for a single map isn't worth it.
#[derive(Debug, Clone)]
pub struct ProjectRegistry {
    root: Arc<PathBuf>,
    inner: Arc<RwLock<HashMap<PathBuf, ProjectMeta>>>,
}

impl ProjectRegistry {
    /// `root` should be an absolute, canonicalized path. The registry uses it
    /// only to relativize the display `path` on each `ProjectMeta` insert; if
    /// callers feed in non-canonical roots, lookups still work but the display
    /// path may include `..` or symlink-shadowed segments.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
            inner: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Absolute, canonicalized root the registry was constructed with.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return a deterministic, sorted-by-relative-path snapshot. We sort so
    /// the SPA's list view is stable across requests; `HashMap` iteration
    /// order is otherwise random.
    pub fn snapshot(&self) -> Vec<ProjectMeta> {
        let guard = self
            .inner
            .read()
            .expect("registry RwLock poisoned by panic in another thread");
        let mut out: Vec<ProjectMeta> = guard.values().cloned().collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Look up a `ProjectMeta` by absolute canonical path. Callers must
    /// canonicalize before calling; the registry does no normalization.
    pub fn get(&self, path: &Path) -> Option<ProjectMeta> {
        let guard = self
            .inner
            .read()
            .expect("registry RwLock poisoned by panic in another thread");
        guard.get(path).cloned()
    }

    /// Insert or replace the entry keyed by `absolute_path`. The stored
    /// `meta.path` is overwritten with the path relativized against the
    /// registry root so callers can pass a partially-populated meta.
    pub fn upsert(&self, absolute_path: PathBuf, mut meta: ProjectMeta) {
        meta.path = relativize(&self.root, &absolute_path);
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        guard.insert(absolute_path, meta);
    }

    /// Insert or update the entry keyed by `absolute_path`, **preserving
    /// the existing version if an entry already exists**.
    ///
    /// The invariant: version is only ever `0` on a brand-new insert;
    /// re-discovering a file that has already been saved must not reset
    /// the optimistic-lock counter back to zero.
    ///
    /// `meta.path` is overwritten with the relativized form (same as
    /// `upsert`). Non-version fields (mtime, size, git) are always
    /// updated to the caller's values.
    pub fn upsert_preserve_version(&self, absolute_path: PathBuf, mut meta: ProjectMeta) {
        meta.path = relativize(&self.root, &absolute_path);
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if let Some(existing) = guard.get(&absolute_path) {
            meta.version = existing.version;
        }
        guard.insert(absolute_path, meta);
    }

    /// Insert or update the entry keyed by `absolute_path`, taking the
    /// **maximum of the incoming version and the existing version**.
    ///
    /// Used in error-recovery paths where the incoming `meta.version` carries
    /// a just-incremented version but the registry may already have a newer
    /// entry (e.g. from a concurrent scan that found a pre-existing sidecar).
    /// Taking the max ensures the optimistic-lock counter never rolls backward
    /// regardless of which write wins the race.
    ///
    /// `meta.path` is overwritten with the relativized form (same as `upsert`).
    pub fn upsert_max_version(&self, absolute_path: PathBuf, mut meta: ProjectMeta) {
        meta.path = relativize(&self.root, &absolute_path);
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if let Some(existing) = guard.get(&absolute_path) {
            meta.version = meta.version.max(existing.version);
        }
        guard.insert(absolute_path, meta);
    }

    /// Get the entry for `absolute_path`, inserting a default if absent.
    ///
    /// The get-or-insert runs under a single write-lock acquisition so
    /// concurrent callers cannot both observe absence and both insert.
    /// The `make_default` closure is only called when no entry exists;
    /// like `upsert`, the returned meta's `path` field is overwritten with
    /// the relativized form.
    ///
    /// Returns the (possibly newly created) entry.
    pub fn ensure_or_get<F>(&self, absolute_path: PathBuf, make_default: F) -> ProjectMeta
    where
        F: FnOnce() -> ProjectMeta,
    {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if !guard.contains_key(&absolute_path) {
            let mut meta = make_default();
            meta.path = relativize(&self.root, &absolute_path);
            guard.insert(absolute_path.clone(), meta);
        }
        guard[&absolute_path].clone()
    }

    /// Remove an entry by absolute canonical path. No-op if the path is not
    /// in the registry.
    pub fn remove(&self, path: &Path) {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        guard.remove(path);
    }

    /// Optimistic-lock primitive: under the write lock, check that
    /// `expected_version` matches the entry's stored version, then increment
    /// it and return the new value.
    ///
    /// The lock is held across the read+compare+increment sequence so two
    /// concurrent calls cannot both observe the same `expected_version` and
    /// both succeed. The lock is released *before* the caller does any file
    /// I/O — the version is "claimed" optimistically. If the subsequent disk
    /// write fails, the registry's version is one ahead of disk content;
    /// this is acceptable for a single-user local tool because a) the
    /// version monotonically increases (no rollback ambiguity), and b) the
    /// next successful save will rewrite the file with the post-increment
    /// version. Phase 3's Loro-doc cache replaces this with a different
    /// concurrency model.
    pub fn check_and_increment(
        &self,
        abs_path: &Path,
        expected_version: u64,
    ) -> Result<u64, RegistryError> {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        let entry = guard.get_mut(abs_path).ok_or(RegistryError::NotFound)?;
        if entry.version != expected_version {
            return Err(RegistryError::VersionMismatch {
                expected: expected_version,
                actual: entry.version,
            });
        }
        entry.version += 1;
        Ok(entry.version)
    }

    /// Update the entry's `mtime` and `size` from a freshly-stat'd
    /// post-write file. No-op if the path is not in the registry.
    ///
    /// Used by the save handler after a successful disk write so a
    /// subsequent listing reflects both the new modification time and
    /// the new file size; the SPA's stale-data heuristics rely on
    /// these to detect external file changes.
    pub fn refresh_meta(&self, abs_path: &Path, mtime: SystemTime, size: u64) {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if let Some(entry) = guard.get_mut(abs_path) {
            entry.mtime = mtime;
            entry.size = size;
        }
    }

    /// Store `hash` as the expected next on-disk fingerprint for `abs_path`
    /// *before* the bytes are written to disk. This closes the race window
    /// between `atomic_write` (which fires an OS watcher event) and a
    /// subsequent `refresh_after_write` call: by the time the watcher sees
    /// the event, `last_disk_hash` already matches, so the event is
    /// echo-suppressed without a spurious `ProjectChanged{Disk}` broadcast.
    ///
    /// No-op if the path is not in the registry.
    pub fn prime_echo_hash(&self, abs_path: &Path, hash: u64) {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if let Some(entry) = guard.get_mut(abs_path) {
            entry.last_disk_hash = hash;
        }
    }

    /// Establish a placeholder sidecar entry mirroring `source_path`'s
    /// data with the on-disk hash primed to `hash`. Used by the
    /// `.mdl → .sd.json` save flow to close the watcher echo-suppression
    /// race window between [`std::fs`] commit and
    /// [`Self::redirect_to_sidecar`].
    ///
    /// The watcher's echo-suppression check looks up the registry by the
    /// canonical path the OS event carries — for a sidecar save that is
    /// the `.sd.json` path, even though the in-memory source-of-truth
    /// entry is keyed on `.mdl` until the redirect runs. Without the
    /// placeholder, a watcher event arriving between `commit_write` and
    /// `redirect_to_sidecar` finds either no entry at all or a
    /// scanner-inserted entry with `last_disk_hash = 0`, falls into the
    /// merge path, and broadcasts a spurious `ProjectChanged{Disk}` for
    /// content the server itself wrote.
    ///
    /// The placeholder shares `source_path`'s doc Arc so reads via
    /// either key (e.g. an HTTP `GET` whose sidecar-preference rule
    /// switched to the sidecar) see the same merged state across the
    /// race window. After [`Self::redirect_to_sidecar`] runs, the
    /// `.mdl` entry is removed and the merged-doc state migrates onto
    /// the sidecar entry directly; the placeholder's data is replaced
    /// (the redirect's `prev` carries forward).
    ///
    /// If a sidecar entry already exists (e.g. the scanner discovered a
    /// pre-existing `.sd.json`), the higher version is kept so an
    /// in-flight client never observes a rollback. Returns
    /// [`RegistryError::NotFound`] when `source_path` is not in the
    /// registry — caller should hold the source entry across this call.
    pub fn prime_sidecar_echo_hash(
        &self,
        source_path: &Path,
        sidecar_path: PathBuf,
        hash: u64,
    ) -> Result<(), RegistryError> {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        let source = guard.get(source_path).ok_or(RegistryError::NotFound)?;
        let mut placeholder = ProjectMeta {
            path: relativize(&self.root, &sidecar_path),
            format: ProjectFormat::SdJson,
            mtime: source.mtime,
            size: source.size,
            git: source.git,
            version: source.version,
            doc: source.doc.clone(),
            last_disk_hash: hash,
            last_diagnostic_keys: source.last_diagnostic_keys.clone(),
        };
        if let Some(existing) = guard.get(&sidecar_path) {
            placeholder.version = placeholder.version.max(existing.version);
        }
        guard.insert(sidecar_path, placeholder);
        Ok(())
    }

    /// Atomic compare-and-update of the entry's `last_diagnostic_keys`
    /// cache.
    ///
    /// Under the registry write lock:
    /// 1. Look up the entry; return `false` if absent (no entry to
    ///    update, and no `DiagnosticsChanged` should fire either).
    /// 2. Compare the cached `last_diagnostic_keys` against `new_keys`.
    ///    Equal → return `false` (caller suppresses the broadcast).
    /// 3. Replace the cached set with `new_keys` and return `true`.
    ///
    /// Why one method instead of an explicit get+compare+set sequence:
    /// the broadcast decision and the cache update must observe the
    /// same snapshot of the entry. A racing save that lands between a
    /// caller's `get` and `upsert` could otherwise produce duplicate
    /// notifications or, worse, a notification that loses to a stale
    /// re-cache.
    ///
    /// The method is fire-and-forget for the missing-entry case. That
    /// matters at startup boundaries (a freshly created entry's
    /// MCP `create` path may publish `ProjectChanged` after the
    /// `upsert_max_version` but before the surrounding code drops the
    /// entry). In practice this is rare; the helper logs nothing
    /// because the absence is benign.
    pub fn update_diagnostic_keys_if_changed(
        &self,
        abs_path: &Path,
        new_keys: &BTreeSet<(String, Option<String>)>,
    ) -> bool {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        let entry = match guard.get_mut(abs_path) {
            Some(e) => e,
            None => return false,
        };
        if entry.last_diagnostic_keys == *new_keys {
            return false;
        }
        entry.last_diagnostic_keys = new_keys.clone();
        true
    }

    /// Atomically update the entry's `git` field, leaving every other
    /// field untouched. Returns `Some(version)` when the value actually
    /// changed (so the caller can broadcast a `ProjectChanged` carrying
    /// the entry's current version), `None` when the entry is absent or
    /// the value is unchanged.
    ///
    /// This is the watcher's `handle_git_change` primitive. The earlier
    /// implementation rebuilt a full `ProjectMeta` from a `snapshot()`
    /// entry and wrote it back via `upsert_preserve_version`, which
    /// preserved only `version` — racing saves landing between
    /// snapshot and upsert would have their freshly-primed
    /// `last_disk_hash` (echo-suppression key) and
    /// `last_diagnostic_keys` (DiagnosticsChanged dedup cache)
    /// overwritten by the snapshot's stale values. Doing the
    /// compare-and-update under one lock against `entry.git` alone
    /// closes that window without requiring the caller to know which
    /// other fields might have moved.
    pub fn update_git_state_if_changed(&self, abs_path: &Path, new_git: GitState) -> Option<u64> {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        let entry = guard.get_mut(abs_path)?;
        if entry.git == new_git {
            return None;
        }
        entry.git = new_git;
        Some(entry.version)
    }

    /// Like `refresh_meta`, but also stores the XXH3-64 hash of the bytes
    /// just written. The hash is the echo-suppression key the Phase 4 file
    /// watcher uses to recognize its own atomic-write events.
    ///
    /// No-op if the path is not in the registry. Called by the save handler
    /// after a successful disk write.
    pub fn refresh_after_write(&self, abs_path: &Path, mtime: SystemTime, size: u64, hash: u64) {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if let Some(entry) = guard.get_mut(abs_path) {
            entry.mtime = mtime;
            entry.size = size;
            entry.last_disk_hash = hash;
        }
    }

    /// Move a `.mdl` entry to its `.sd.json` sidecar key after the
    /// sidecar is created on disk. Carries the version forward (so an
    /// in-flight optimistic-lock conversation isn't broken by the
    /// sidecar transition) and switches the format to `SdJson`. The
    /// `.mdl` key is dropped from the registry.
    ///
    /// This rule encodes the design's "sidecar becomes source of truth
    /// once it exists" semantics at the registry layer: subsequent
    /// reads (via the GET handler's preference rule, or via the
    /// registry directly) will see the sidecar, not the `.mdl`.
    ///
    /// Returns `RegistryError::NotFound` when no entry exists at
    /// `mdl_path`. The whole transition runs under the write lock so
    /// it's atomic w.r.t. concurrent saves.
    pub fn redirect_to_sidecar(
        &self,
        mdl_path: &Path,
        sidecar_path: PathBuf,
    ) -> Result<(), RegistryError> {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        let prev = guard.remove(mdl_path).ok_or(RegistryError::NotFound)?;
        // If the scanner already discovered a pre-existing sidecar and inserted
        // an entry for it, take the higher version so no in-flight client ever
        // sees a rollback.
        let version = match guard.get(&sidecar_path) {
            Some(existing) => prev.version.max(existing.version),
            None => prev.version,
        };
        let new_meta = ProjectMeta {
            path: relativize(&self.root, &sidecar_path),
            format: ProjectFormat::SdJson,
            mtime: prev.mtime,
            size: prev.size,
            git: prev.git,
            version,
            // Carry the just-merged doc forward to the sidecar key so
            // the post-redirect cache reflects the saved state without
            // a re-parse. This complements Task 8's
            // `check_increment_and_merge`: the merge runs against the
            // .mdl-keyed doc just before this redirect runs, so the
            // doc Arc here already holds the post-merge state.
            doc: prev.doc,
            // Disk-write fingerprint follows the canonical-form path. The
            // .mdl entry's hash (typically 0) is meaningless once the
            // sidecar takes over as source of truth; the next save will
            // refresh this slot with the sidecar's bytes.
            last_disk_hash: prev.last_disk_hash,
            // Carry the cached diagnostic key set forward so a save that
            // simply transitions .mdl → .sd.json without changing the
            // model body doesn't re-emit `DiagnosticsChanged`.
            last_diagnostic_keys: prev.last_diagnostic_keys,
        };
        guard.insert(sidecar_path, new_meta);
        Ok(())
    }

    /// Move the entry at `from` to `to` without re-hydrating its doc or
    /// resetting any path-independent state. The optimistic-lock version,
    /// echo-suppression hash, cached diagnostic keys, and `Arc<ProjectDoc>`
    /// are all preserved verbatim across the re-key. Only the relativized
    /// display path is recomputed against the registry root.
    ///
    /// Returns `RegistryError::NotFound` if no entry exists at `from`.
    /// Returns `RegistryError::AlreadyExists` if an entry is already tracked
    /// at `to`: the caller is responsible for deciding whether to displace
    /// the existing entry (e.g. by removing it and broadcasting
    /// `ProjectRemoved` first) rather than silently overwriting its state.
    /// The whole transition runs under the write lock so it's atomic
    /// w.r.t. concurrent saves and watcher events.
    ///
    /// The `format` field is also preserved as-is — a rename across
    /// extensions does not re-classify; callers that intend to switch
    /// formats should `upsert` the new meta directly.
    pub fn rename_entry(&self, from: &Path, to: &Path) -> Result<(), RegistryError> {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if guard.contains_key(to) {
            return Err(RegistryError::AlreadyExists);
        }
        let mut entry = guard.remove(from).ok_or(RegistryError::NotFound)?;
        entry.path = relativize(&self.root, to);
        guard.insert(to.to_path_buf(), entry);
        Ok(())
    }

    /// Combined optimistic-lock version check, version increment, and
    /// `apply_canonical_json` merge against the entry's `ProjectDoc`.
    ///
    /// This is the fused primitive Task 8 introduces: instead of
    /// `check_and_increment` (registry only) followed by a separate
    /// disk-write step that left the in-memory doc out of date, the
    /// save handler now drives every write through here. The single
    /// registry-write-lock acquisition wraps:
    ///
    /// 1. Look up the entry; return `NotFound` when absent.
    /// 2. Verify `expected_version` matches; return `VersionMismatch`
    ///    otherwise (the failing client refetches against `actual`).
    /// 3. Lazily hydrate the entry's `ProjectDoc` if it isn't already
    ///    cached (mirrors `get_or_init_doc` but inlined since we
    ///    already hold the registry lock).
    /// 4. Call `apply_canonical_json` on the doc with the new state.
    /// 5. Bump the entry's version.
    ///
    /// Lock scope: holding the registry write lock through the merge
    /// is the documented trade-off (plan Notes 5/11). Within one
    /// process the LoroDoc has interior mutability and no internal
    /// lock, so we serialize all writes ourselves; doing it under the
    /// registry lock avoids needing a second tier of per-entry write
    /// locks. Disk I/O (the actual file write) happens outside this
    /// method, against the returned `Arc<ProjectDoc>`.
    ///
    /// On success returns the new version paired with the Arc'd doc so
    /// the caller can serialize and stat the disk file outside the
    /// lock.
    pub fn check_increment_and_merge(
        &self,
        abs_path: &Path,
        expected_version: u64,
        new_json: &serde_json::Value,
    ) -> Result<(u64, Arc<ProjectDoc>), RegistryError> {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        let entry = guard.get_mut(abs_path).ok_or(RegistryError::NotFound)?;
        if entry.version != expected_version {
            return Err(RegistryError::VersionMismatch {
                expected: expected_version,
                actual: entry.version,
            });
        }

        // Resolve the doc Arc, hydrating from disk if this is the first
        // touch for this entry. The `doc` slot's RwLock is independent
        // of the registry's RwLock, so taking it while we hold the
        // registry write lock can't deadlock — they protect disjoint
        // state.
        let format = entry.format;
        let doc_slot = entry.doc.clone();
        let arc_doc = {
            let read_guard = doc_slot
                .read()
                .expect("doc RwLock poisoned by panic in another thread");
            if let Some(existing) = read_guard.as_ref() {
                existing.clone()
            } else {
                drop(read_guard);
                let mut write_guard = doc_slot
                    .write()
                    .expect("doc RwLock poisoned by panic in another thread");
                if let Some(existing) = write_guard.as_ref() {
                    existing.clone()
                } else {
                    let doc = hydrate_doc_from_disk(abs_path, format)?;
                    let arc = Arc::new(doc);
                    *write_guard = Some(arc.clone());
                    arc
                }
            }
        };

        arc_doc
            .apply_canonical_json(new_json)
            .map_err(|e| RegistryError::HydrationFailed(format!("apply: {e}")))?;
        entry.version += 1;
        let new_version = entry.version;
        Ok((new_version, arc_doc))
    }

    /// Apply a merge sourced from an on-disk change without a client-supplied
    /// version check. Mirrors `check_increment_and_merge` minus the
    /// version-comparison step: the file watcher is authoritative for "what
    /// just happened on disk", so there's no expected_version to honor.
    ///
    /// On every call we:
    /// 1. Look up the entry (returning `NotFound` when absent).
    /// 2. Lazily hydrate the entry's `ProjectDoc` if it isn't already cached.
    /// 3. Apply `apply_canonical_json` against the doc.
    /// 4. Bump the entry's version.
    ///
    /// Returns the new version on success. Lock scope and `doc` slot
    /// hydration follow the same shape as `check_increment_and_merge`; the
    /// only structural difference is the absence of a version-mismatch arm.
    pub fn merge_disk_change(
        &self,
        abs_path: &Path,
        new_json: &serde_json::Value,
    ) -> Result<u64, RegistryError> {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        let entry = guard.get_mut(abs_path).ok_or(RegistryError::NotFound)?;

        let format = entry.format;
        let doc_slot = entry.doc.clone();
        let arc_doc = {
            let read_guard = doc_slot
                .read()
                .expect("doc RwLock poisoned by panic in another thread");
            if let Some(existing) = read_guard.as_ref() {
                existing.clone()
            } else {
                drop(read_guard);
                let mut write_guard = doc_slot
                    .write()
                    .expect("doc RwLock poisoned by panic in another thread");
                if let Some(existing) = write_guard.as_ref() {
                    existing.clone()
                } else {
                    let doc = hydrate_doc_from_disk(abs_path, format)?;
                    let arc = Arc::new(doc);
                    *write_guard = Some(arc.clone());
                    arc
                }
            }
        };

        arc_doc
            .apply_canonical_json(new_json)
            .map_err(|e| RegistryError::HydrationFailed(format!("apply: {e}")))?;
        entry.version += 1;
        Ok(entry.version)
    }

    /// Number of entries currently in the registry.
    pub fn len(&self) -> usize {
        let guard = self
            .inner
            .read()
            .expect("registry RwLock poisoned by panic in another thread");
        guard.len()
    }

    /// True iff the registry has no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lazily construct the in-memory `ProjectDoc` for `abs_path`.
    ///
    /// First call: reads the file, parses it through the existing
    /// extension-driven dispatcher, converts to canonical JSON, then
    /// applies that JSON into a fresh `ProjectDoc`. The result is
    /// cached on the entry's `doc` slot and returned.
    ///
    /// Subsequent calls return the cached `Arc<ProjectDoc>` without
    /// touching disk — this is the key property that makes the doc the
    /// source of truth for reads after first access.
    ///
    /// Returns:
    /// - `RegistryError::NotFound` if no entry exists for `abs_path`.
    /// - `RegistryError::HydrationFailed(msg)` if the file is missing,
    ///   unreadable, or parses to something that doesn't fit the
    ///   project schema.
    ///
    /// The hydration path holds the entry's `doc` write lock across the
    /// disk read + parse + apply. This is fine for first-touch (the lock
    /// is uncontended) and for the "two callers race the first hydration"
    /// case the second waits for the first to finish, then sees the cached
    /// `Some` and returns immediately.
    pub fn get_or_init_doc(&self, abs_path: &Path) -> Result<Arc<ProjectDoc>, RegistryError> {
        // Look up the entry's doc slot. We clone the outer Arc here so
        // the registry's HashMap lock is released before we hold the
        // per-entry doc lock — keeps the registry-wide lock as short as
        // possible (other reads/writes against unrelated entries can
        // proceed concurrently).
        let doc_slot = {
            let guard = self
                .inner
                .read()
                .expect("registry RwLock poisoned by panic in another thread");
            let entry = guard.get(abs_path).ok_or(RegistryError::NotFound)?;
            entry.doc.clone()
        };

        // Fast path: doc already hydrated.
        {
            let read_guard = doc_slot
                .read()
                .expect("doc RwLock poisoned by panic in another thread");
            if let Some(existing) = read_guard.as_ref() {
                return Ok(existing.clone());
            }
        }

        // Slow path: hydrate under the entry's write lock. The registry
        // lookup above only told us *whether* the doc was hydrated; once
        // we have the write lock we re-check, since another caller may
        // have raced us between the read-unlock and the write-acquire.
        let mut write_guard = doc_slot
            .write()
            .expect("doc RwLock poisoned by panic in another thread");
        if let Some(existing) = write_guard.as_ref() {
            return Ok(existing.clone());
        }

        // Need the entry's format to dispatch the parser. Re-read it
        // here because the entry's other fields (mtime, size) may have
        // changed since the initial lookup; we want the format actually
        // recorded right now.
        let format = {
            let guard = self
                .inner
                .read()
                .expect("registry RwLock poisoned by panic in another thread");
            guard.get(abs_path).ok_or(RegistryError::NotFound)?.format
        };

        let doc = hydrate_doc_from_disk(abs_path, format)?;
        let arc_doc = Arc::new(doc);
        *write_guard = Some(arc_doc.clone());
        Ok(arc_doc)
    }
}

/// Read `path`, parse it via the project format dispatcher, convert
/// to canonical JSON, and apply that into a fresh `ProjectDoc`. Errors
/// (file missing, parse failure, merge failure) all surface as
/// `RegistryError::HydrationFailed` with a human-readable message.
fn hydrate_doc_from_disk(path: &Path, format: ProjectFormat) -> Result<ProjectDoc, RegistryError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| RegistryError::HydrationFailed(format!("read {}: {e}", path.display())))?;
    let project = parse_to_datamodel(path, format, &contents).map_err(|e: ParseError| {
        RegistryError::HydrationFailed(format!("parse {}: {e}", path.display()))
    })?;
    let json_project: simlin_engine::json::Project = (&project).into();
    let json_value = serde_json::to_value(&json_project)
        .map_err(|e| RegistryError::HydrationFailed(format!("serialize project: {e}")))?;
    let doc = ProjectDoc::new();
    doc.apply_canonical_json(&json_value)
        .map_err(|e: MergeError| RegistryError::HydrationFailed(format!("apply: {e}")))?;
    Ok(doc)
}

/// Compute `absolute.strip_prefix(root)`, falling back to the full absolute
/// path if the prefix doesn't apply. The fallback exists for robustness:
/// callers shouldn't trip it in practice, but if they do, displaying the full
/// absolute path is more useful than panicking.
fn relativize(root: &Path, absolute: &Path) -> PathBuf {
    absolute
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| absolute.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_meta(path: PathBuf, format: ProjectFormat) -> ProjectMeta {
        ProjectMeta {
            path,
            format,
            mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            size: 42,
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
            last_diagnostic_keys: BTreeSet::new(),
        }
    }

    #[test]
    fn empty_registry_returns_empty_snapshot() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        assert!(reg.is_empty());
        assert_eq!(reg.snapshot().len(), 0);
    }

    #[test]
    fn snapshot_returns_entries_sorted_by_relative_path() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());

        let a_abs = root.join("zebra.stmx");
        let b_abs = root.join("apple.stmx");
        let c_abs = root.join("sub").join("middle.stmx");

        reg.upsert(
            a_abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx),
        );
        reg.upsert(
            b_abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx),
        );
        reg.upsert(
            c_abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx),
        );

        let snap = reg.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].path, PathBuf::from("apple.stmx"));
        assert_eq!(snap[1].path, PathBuf::from("sub").join("middle.stmx"));
        assert_eq!(snap[2].path, PathBuf::from("zebra.stmx"));
    }

    #[test]
    fn get_finds_entries_by_absolute_canonical_key() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.xmile");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Xmile),
        );

        let found = reg.get(&abs).expect("entry should be present");
        assert_eq!(found.path, PathBuf::from("model.xmile"));
        assert_eq!(found.format, ProjectFormat::Xmile);
    }

    #[test]
    fn get_returns_none_for_unknown_paths() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        assert!(reg.get(Path::new("/tmp/root/missing.stmx")).is_none());
    }

    #[test]
    fn remove_drops_the_entry() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.mdl");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Mdl),
        );
        assert_eq!(reg.len(), 1);

        reg.remove(&abs);
        assert!(reg.is_empty());
        assert!(reg.get(&abs).is_none());
    }

    #[test]
    fn upsert_overwrites_existing_entry() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        reg.upsert(
            abs.clone(),
            ProjectMeta {
                path: PathBuf::new(),
                format: ProjectFormat::Stmx,
                mtime: SystemTime::UNIX_EPOCH,
                size: 1,
                git: GitState::Untracked,
                version: 0,
                doc: Default::default(),
                last_disk_hash: 0,
                last_diagnostic_keys: BTreeSet::new(),
            },
        );
        reg.upsert(
            abs.clone(),
            ProjectMeta {
                path: PathBuf::new(),
                format: ProjectFormat::Stmx,
                mtime: SystemTime::UNIX_EPOCH,
                size: 999,
                git: GitState::Tracked { dirty: true },
                version: 0,
                doc: Default::default(),
                last_disk_hash: 0,
                last_diagnostic_keys: BTreeSet::new(),
            },
        );

        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.size, 999);
        assert_eq!(entry.git, GitState::Tracked { dirty: true });
    }

    #[test]
    fn project_format_serializes_as_lowercase() {
        assert_eq!(
            serde_json::to_string(&ProjectFormat::Stmx).unwrap(),
            "\"stmx\""
        );
        assert_eq!(
            serde_json::to_string(&ProjectFormat::SdJson).unwrap(),
            "\"sd_json\""
        );
    }

    #[test]
    fn git_state_serializes_with_kind_tag() {
        let tracked = serde_json::to_value(GitState::Tracked { dirty: true }).unwrap();
        assert_eq!(tracked["kind"], "tracked");
        assert_eq!(tracked["dirty"], true);

        let untracked = serde_json::to_value(GitState::Untracked).unwrap();
        assert_eq!(untracked["kind"], "untracked");

        let unavail = serde_json::to_value(GitState::Unavailable).unwrap();
        assert_eq!(unavail["kind"], "unavailable");
    }

    #[test]
    fn project_format_display_uses_lowercase_strings() {
        assert_eq!(format!("{}", ProjectFormat::Stmx), "stmx");
        assert_eq!(format!("{}", ProjectFormat::Xmile), "xmile");
        assert_eq!(format!("{}", ProjectFormat::Mdl), "mdl");
        assert_eq!(format!("{}", ProjectFormat::SdJson), "sd_json");
    }

    #[test]
    fn check_and_increment_returns_not_found_for_unknown_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let err = reg
            .check_and_increment(Path::new("/tmp/root/missing.stmx"), 0)
            .unwrap_err();
        assert_eq!(err, RegistryError::NotFound);
    }

    #[test]
    fn check_and_increment_increments_on_match() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.version = 5;
        reg.upsert(abs.clone(), meta);

        let new_version = reg.check_and_increment(&abs, 5).expect("matches");
        assert_eq!(new_version, 6);
        assert_eq!(reg.get(&abs).expect("entry").version, 6);
    }

    #[test]
    fn check_and_increment_rejects_stale_version() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.version = 5;
        reg.upsert(abs.clone(), meta);

        // First call increments 5 -> 6.
        reg.check_and_increment(&abs, 5).expect("first match");
        // Second call with the now-stale `5` must fail and report `actual: 6`.
        let err = reg.check_and_increment(&abs, 5).unwrap_err();
        assert_eq!(
            err,
            RegistryError::VersionMismatch {
                expected: 5,
                actual: 6
            }
        );
        // The registry must not have incremented further on the failed attempt.
        assert_eq!(reg.get(&abs).expect("entry").version, 6);
    }

    #[test]
    fn redirect_to_sidecar_moves_entry_and_carries_version() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl_abs = root.join("model.mdl");
        let sidecar_abs = root.join("model.sd.json");

        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Mdl);
        meta.version = 7;
        reg.upsert(mdl_abs.clone(), meta);

        reg.redirect_to_sidecar(&mdl_abs, sidecar_abs.clone())
            .expect("redirect succeeds");

        // The .mdl key is gone.
        assert!(reg.get(&mdl_abs).is_none());
        // The sidecar key holds the new format with version carried over.
        let entry = reg.get(&sidecar_abs).expect("sidecar entry");
        assert_eq!(entry.format, ProjectFormat::SdJson);
        assert_eq!(entry.version, 7);
        // The display path is relativized against the registry root.
        assert_eq!(entry.path, PathBuf::from("model.sd.json"));
    }

    #[test]
    fn redirect_to_sidecar_returns_not_found_for_unknown_mdl() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let mdl = PathBuf::from("/tmp/root/missing.mdl");
        let sidecar = PathBuf::from("/tmp/root/missing.sd.json");
        let err = reg.redirect_to_sidecar(&mdl, sidecar).unwrap_err();
        assert_eq!(err, RegistryError::NotFound);
    }

    #[test]
    fn refresh_meta_updates_mtime_and_size() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx),
        );
        let new_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let new_size = 9_999u64;
        reg.refresh_meta(&abs, new_mtime, new_size);
        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.mtime, new_mtime);
        assert_eq!(entry.size, new_size);
    }

    #[test]
    fn refresh_meta_is_noop_for_missing_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let nonexistent = PathBuf::from("/tmp/root/missing.stmx");
        reg.refresh_meta(
            &nonexistent,
            SystemTime::UNIX_EPOCH + Duration::from_secs(42),
            123,
        );
        assert!(reg.is_empty());
    }

    #[test]
    fn refresh_after_write_stores_hash_alongside_mtime_and_size() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx),
        );
        let new_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let new_size = 9_999u64;
        let new_hash = 0xdead_beef_dead_beefu64;
        reg.refresh_after_write(&abs, new_mtime, new_size, new_hash);
        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.mtime, new_mtime);
        assert_eq!(entry.size, new_size);
        assert_eq!(entry.last_disk_hash, new_hash);
    }

    #[test]
    fn update_diagnostic_keys_if_changed_returns_false_for_missing_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let nonexistent = PathBuf::from("/tmp/root/missing.stmx");
        let mut keys = BTreeSet::new();
        keys.insert(("syntax".to_string(), Some("x".to_string())));
        assert!(!reg.update_diagnostic_keys_if_changed(&nonexistent, &keys));
    }

    #[test]
    fn update_diagnostic_keys_if_changed_returns_false_when_set_equals_cached() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        let mut cached = BTreeSet::new();
        cached.insert(("syntax".to_string(), Some("x".to_string())));
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.last_diagnostic_keys = cached.clone();
        reg.upsert(abs.clone(), meta);

        assert!(!reg.update_diagnostic_keys_if_changed(&abs, &cached));
        // The cached set must still equal the original (no clobber).
        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.last_diagnostic_keys, cached);
    }

    #[test]
    fn update_diagnostic_keys_if_changed_returns_true_when_set_differs_and_swaps() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        let cached = BTreeSet::new();
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.last_diagnostic_keys = cached;
        reg.upsert(abs.clone(), meta);

        let mut new_keys = BTreeSet::new();
        new_keys.insert(("unknown_dependency".to_string(), Some("bad".to_string())));
        assert!(reg.update_diagnostic_keys_if_changed(&abs, &new_keys));
        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.last_diagnostic_keys, new_keys);
    }

    #[test]
    fn update_diagnostic_keys_if_changed_swaps_back_to_empty() {
        // The "all errors fixed" transition: cached has entries, new is
        // empty. The method must report changed and replace the cached
        // set with the empty one so the next invocation reports
        // unchanged.
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        let mut cached = BTreeSet::new();
        cached.insert(("syntax".to_string(), Some("x".to_string())));
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.last_diagnostic_keys = cached;
        reg.upsert(abs.clone(), meta);

        let empty = BTreeSet::new();
        assert!(reg.update_diagnostic_keys_if_changed(&abs, &empty));
        let entry = reg.get(&abs).expect("entry");
        assert!(entry.last_diagnostic_keys.is_empty());

        // Second call with the same empty set: no-op.
        assert!(!reg.update_diagnostic_keys_if_changed(&abs, &empty));
    }

    #[test]
    fn refresh_after_write_is_noop_for_missing_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let nonexistent = PathBuf::from("/tmp/root/missing.stmx");
        reg.refresh_after_write(
            &nonexistent,
            SystemTime::UNIX_EPOCH + Duration::from_secs(42),
            123,
            0xfeed_face_feed_faceu64,
        );
        assert!(reg.is_empty());
    }

    #[test]
    fn check_and_increment_is_serialized_under_concurrency() {
        // Two threads both attempting `check_and_increment(_, 5)` must see
        // exactly one Ok and one VersionMismatch. The Barrier synchronizes
        // the start so the race is real, not sequential.
        use std::sync::Barrier;
        use std::thread;

        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("contended.stmx");
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.version = 5;
        reg.upsert(abs.clone(), meta);

        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::with_capacity(2);
        for _ in 0..2 {
            let reg = reg.clone();
            let abs = abs.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                reg.check_and_increment(&abs, 5)
            }));
        }
        let results: Vec<Result<u64, RegistryError>> =
            handles.into_iter().map(|h| h.join().unwrap()).collect();

        let oks: Vec<_> = results.iter().filter(|r| r.is_ok()).collect();
        let errs: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
        assert_eq!(oks.len(), 1, "exactly one thread must succeed");
        assert_eq!(errs.len(), 1, "exactly one thread must observe stale");
        assert_eq!(*oks[0].as_ref().unwrap(), 6u64);
        assert_eq!(
            *errs[0].as_ref().unwrap_err(),
            RegistryError::VersionMismatch {
                expected: 5,
                actual: 6
            }
        );
        // Final state: version is exactly 6 (not 7).
        assert_eq!(reg.get(&abs).expect("entry").version, 6);
    }

    #[test]
    fn upsert_preserve_version_keeps_existing_version_on_update() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");

        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.version = 3;
        reg.upsert(abs.clone(), meta);

        // Rescan-like call: version in the meta is 0 (scanner always sets 0),
        // but upsert_preserve_version must keep the existing 3.
        let mut rescan_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        rescan_meta.size = 999;
        rescan_meta.version = 0;
        reg.upsert_preserve_version(abs.clone(), rescan_meta);

        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.version, 3, "version must not be reset on rescan");
        assert_eq!(entry.size, 999, "non-version fields must be updated");
    }

    #[test]
    fn upsert_preserve_version_inserts_with_version_zero_when_new() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("new.stmx");

        // Fresh entry: no existing record, so version from the meta is used.
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.version = 0;
        reg.upsert_preserve_version(abs.clone(), meta);

        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.version, 0);
    }

    #[test]
    fn update_git_state_if_changed_preserves_disk_hash_and_diagnostics() {
        // Watcher's handle_git_change used to rebuild a full ProjectMeta from
        // a snapshot and write it back via upsert_preserve_version, which
        // clobbered last_disk_hash and last_diagnostic_keys when a save
        // landed between the snapshot and the write. The CAS-style helper
        // mutates only `git`, leaving every other field untouched even
        // under racing writes.
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");

        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.version = 11;
        meta.last_disk_hash = 0xdead_beef_cafe_babe;
        let mut keys = BTreeSet::new();
        keys.insert(("UnknownDependency".to_string(), Some("x".to_string())));
        meta.last_diagnostic_keys = keys.clone();
        reg.upsert(abs.clone(), meta);

        let returned = reg.update_git_state_if_changed(&abs, GitState::Tracked { dirty: true });
        assert_eq!(
            returned,
            Some(11),
            "must report the entry's current version when git state changed"
        );

        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.git, GitState::Tracked { dirty: true });
        assert_eq!(entry.version, 11, "version must not be touched");
        assert_eq!(
            entry.last_disk_hash, 0xdead_beef_cafe_babe,
            "echo-suppression hash must not be clobbered by a git update"
        );
        assert_eq!(
            entry.last_diagnostic_keys, keys,
            "diagnostic key cache must not be clobbered by a git update"
        );
    }

    #[test]
    fn update_git_state_if_changed_returns_none_when_unchanged() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");

        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.git = GitState::Tracked { dirty: false };
        reg.upsert(abs.clone(), meta);

        let returned = reg.update_git_state_if_changed(&abs, GitState::Tracked { dirty: false });
        assert_eq!(
            returned, None,
            "no broadcast/version-emit when the git state is unchanged"
        );
    }

    #[test]
    fn update_git_state_if_changed_returns_none_when_path_absent() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let returned =
            reg.update_git_state_if_changed(&root.join("nope.stmx"), GitState::Untracked);
        assert_eq!(returned, None);
    }

    #[test]
    fn prime_sidecar_echo_hash_creates_placeholder_with_shared_doc() {
        // The save handler primes a placeholder sidecar entry before
        // commit_write fires the OS event for the freshly-written
        // .sd.json. The placeholder must (a) carry the primed hash so
        // the watcher's lookup-by-sidecar-path echo-suppresses, and
        // (b) share the source's doc Arc so reads via either path
        // observe the same in-memory state until redirect_to_sidecar
        // collapses them.
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl = root.join("model.mdl");
        let sidecar = root.join("model.sd.json");

        let mut mdl_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Mdl);
        mdl_meta.version = 5;
        let mut keys = BTreeSet::new();
        keys.insert(("UnknownDependency".to_string(), Some("x".to_string())));
        mdl_meta.last_diagnostic_keys = keys.clone();
        reg.upsert(mdl.clone(), mdl_meta);

        reg.prime_sidecar_echo_hash(&mdl, sidecar.clone(), 0xCAFE_BEEF)
            .expect("prime succeeds when source exists");

        let mdl_entry = reg.get(&mdl).expect("mdl entry preserved");
        let sidecar_entry = reg.get(&sidecar).expect("sidecar placeholder created");

        assert_eq!(sidecar_entry.last_disk_hash, 0xCAFE_BEEF);
        assert_eq!(sidecar_entry.format, ProjectFormat::SdJson);
        assert_eq!(sidecar_entry.version, 5, "version mirrors source");
        assert_eq!(
            sidecar_entry.last_diagnostic_keys, keys,
            "diagnostic-keys cache mirrors source so DiagnosticsChanged dedup behaves correctly"
        );
        assert!(
            Arc::ptr_eq(&mdl_entry.doc, &sidecar_entry.doc),
            "doc Arc must be shared so concurrent reads via either path see the same merged state"
        );
    }

    #[test]
    fn prime_sidecar_echo_hash_returns_not_found_when_source_missing() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let result = reg.prime_sidecar_echo_hash(
            &root.join("missing.mdl"),
            root.join("missing.sd.json"),
            0xAA,
        );
        assert!(matches!(result, Err(RegistryError::NotFound)));
    }

    #[test]
    fn prime_sidecar_echo_hash_takes_max_version_with_existing_sidecar() {
        // Pre-existing sidecar entry (e.g. scanner found it before the save
        // started). The placeholder must carry the higher version forward
        // so an in-flight client never observes a rollback.
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl = root.join("model.mdl");
        let sidecar = root.join("model.sd.json");

        let mut mdl_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Mdl);
        mdl_meta.version = 3;
        reg.upsert(mdl.clone(), mdl_meta);

        let mut existing = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        existing.version = 9;
        reg.upsert(sidecar.clone(), existing);

        reg.prime_sidecar_echo_hash(&mdl, sidecar.clone(), 0x1234)
            .expect("prime succeeds");

        let entry = reg.get(&sidecar).expect("sidecar entry");
        assert_eq!(
            entry.version, 9,
            "must take max(3, 9) = 9 to avoid rollback"
        );
        assert_eq!(entry.last_disk_hash, 0x1234);
    }

    #[test]
    fn redirect_to_sidecar_preserves_max_version_when_sidecar_already_exists() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl_abs = root.join("model.mdl");
        let sidecar_abs = root.join("model.sd.json");

        // Both entries exist with different versions (can happen when the
        // scanner finds a pre-existing sidecar before the sidecar-redirect
        // path runs). The new entry should take the higher version so
        // in-flight clients never see a version rollback.
        let mut mdl_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Mdl);
        mdl_meta.version = 4;
        reg.upsert(mdl_abs.clone(), mdl_meta);

        let mut sidecar_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        sidecar_meta.version = 7;
        reg.upsert(sidecar_abs.clone(), sidecar_meta);

        reg.redirect_to_sidecar(&mdl_abs, sidecar_abs.clone())
            .expect("redirect succeeds");

        // .mdl key dropped.
        assert!(reg.get(&mdl_abs).is_none());
        // Sidecar gets max(4, 7) = 7.
        let entry = reg.get(&sidecar_abs).expect("sidecar entry");
        assert_eq!(entry.version, 7);
        assert_eq!(entry.format, ProjectFormat::SdJson);
    }

    #[test]
    fn redirect_to_sidecar_preserves_max_version_when_mdl_greater_than_sidecar() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl_abs = root.join("model.mdl");
        let sidecar_abs = root.join("model.sd.json");

        let mut mdl_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Mdl);
        mdl_meta.version = 7;
        reg.upsert(mdl_abs.clone(), mdl_meta);

        let mut sidecar_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        sidecar_meta.version = 4;
        reg.upsert(sidecar_abs.clone(), sidecar_meta);

        reg.redirect_to_sidecar(&mdl_abs, sidecar_abs.clone())
            .expect("redirect succeeds");

        assert!(reg.get(&mdl_abs).is_none());
        // Sidecar gets max(7, 4) = 7.
        let entry = reg.get(&sidecar_abs).expect("sidecar entry");
        assert_eq!(entry.version, 7);
        assert_eq!(entry.format, ProjectFormat::SdJson);
    }

    #[test]
    fn upsert_max_version_takes_max_of_incoming_and_existing() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.sd.json");

        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        meta.version = 10;
        reg.upsert(abs.clone(), meta);

        // Incoming version is lower: existing (10) must be kept.
        let mut lower_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        lower_meta.version = 5;
        reg.upsert_max_version(abs.clone(), lower_meta);
        assert_eq!(reg.get(&abs).expect("entry").version, 10);

        // Incoming version is higher: new version (15) must be kept.
        let mut higher_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        higher_meta.version = 15;
        reg.upsert_max_version(abs.clone(), higher_meta);
        assert_eq!(reg.get(&abs).expect("entry").version, 15);
    }

    #[test]
    fn upsert_max_version_inserts_with_incoming_version_when_new() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("new.sd.json");

        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        meta.version = 3;
        reg.upsert_max_version(abs.clone(), meta);

        assert_eq!(reg.get(&abs).expect("entry").version, 3);
    }

    // The next two tests model the save handler's redirect_to_sidecar success
    // and failure paths (handlers.rs lines ~408-444).

    #[test]
    fn handler_redirect_success_path_version_carries_over() {
        // Simulate: .mdl entry exists at version 5; post-write redirect succeeds.
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl_abs = root.join("model.mdl");
        let sidecar_abs = root.join("model.sd.json");

        let mut mdl_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Mdl);
        mdl_meta.version = 5;
        reg.upsert(mdl_abs.clone(), mdl_meta);

        let result = reg.redirect_to_sidecar(&mdl_abs, sidecar_abs.clone());
        assert!(
            result.is_ok(),
            "redirect must succeed when .mdl entry exists"
        );

        assert!(reg.get(&mdl_abs).is_none(), ".mdl entry must be removed");
        let entry = reg.get(&sidecar_abs).expect("sidecar entry created");
        assert_eq!(entry.version, 5, "version must carry over from .mdl");
        assert_eq!(entry.format, ProjectFormat::SdJson);
    }

    #[test]
    fn handler_redirect_failure_path_fallback_upsert_max_version() {
        // Simulate: the .mdl entry was removed by a concurrent scan between the
        // version-lock release and the post-write redirect call. The handler
        // catches the NotFound error and falls back to upsert_max_version with
        // the just-incremented version so the sidecar is tracked.
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl_abs = root.join("model.mdl");
        let sidecar_abs = root.join("model.sd.json");

        // The .mdl entry is gone by the time redirect_to_sidecar runs.
        let err = reg
            .redirect_to_sidecar(&mdl_abs, sidecar_abs.clone())
            .unwrap_err();
        assert_eq!(err, RegistryError::NotFound);

        // Handler fallback: upsert_max_version with the new (just-incremented)
        // version. This must make the sidecar visible in the registry.
        let new_version = 6u64;
        reg.upsert_max_version(
            sidecar_abs.clone(),
            ProjectMeta {
                path: PathBuf::new(),
                format: ProjectFormat::SdJson,
                mtime: std::time::SystemTime::UNIX_EPOCH,
                size: 0,
                git: GitState::Untracked,
                version: new_version,
                doc: Default::default(),
                last_disk_hash: 0,
                last_diagnostic_keys: BTreeSet::new(),
            },
        );

        let entry = reg
            .get(&sidecar_abs)
            .expect("sidecar entry must exist after fallback");
        assert_eq!(entry.version, new_version);
        assert_eq!(entry.format, ProjectFormat::SdJson);
    }

    #[test]
    fn handler_redirect_failure_fallback_preserves_higher_concurrent_version() {
        // Edge case: redirect_to_sidecar fails (no .mdl), but a concurrent scan
        // already inserted a sidecar entry with a higher version. The fallback
        // upsert_max_version must not roll the version backward.
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let mdl_abs = root.join("model.mdl");
        let sidecar_abs = root.join("model.sd.json");

        // Scanner pre-inserted a sidecar at version 20.
        let mut sidecar_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        sidecar_meta.version = 20;
        reg.upsert(sidecar_abs.clone(), sidecar_meta);

        // redirect_to_sidecar fails because .mdl is absent.
        let err = reg
            .redirect_to_sidecar(&mdl_abs, sidecar_abs.clone())
            .unwrap_err();
        assert_eq!(err, RegistryError::NotFound);

        // Fallback: new_version = 7 (the just-incremented value from
        // check_and_increment), but the existing sidecar is already at 20.
        let new_version = 7u64;
        reg.upsert_max_version(
            sidecar_abs.clone(),
            ProjectMeta {
                path: PathBuf::new(),
                format: ProjectFormat::SdJson,
                mtime: std::time::SystemTime::UNIX_EPOCH,
                size: 0,
                git: GitState::Untracked,
                version: new_version,
                doc: Default::default(),
                last_disk_hash: 0,
                last_diagnostic_keys: BTreeSet::new(),
            },
        );

        // Version must remain 20 (max(7, 20)), not roll back to 7.
        let entry = reg.get(&sidecar_abs).expect("sidecar entry");
        assert_eq!(entry.version, 20, "version must not roll backward");
    }

    #[test]
    fn get_or_init_doc_returns_not_found_for_unknown_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let err = reg
            .get_or_init_doc(Path::new("/tmp/root/missing.stmx"))
            .unwrap_err();
        assert_eq!(err, RegistryError::NotFound);
    }

    #[test]
    fn get_or_init_doc_hydration_fails_with_useful_message_when_file_missing() {
        // Insert an entry pointing at a path that doesn't exist on disk.
        // Hydration must surface HydrationFailed with a message naming
        // the missing path so the API layer can surface it usefully.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let reg = ProjectRegistry::new(temp.path().to_path_buf());
        let abs = temp.path().join("does-not-exist.stmx");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx),
        );

        let err = reg.get_or_init_doc(&abs).unwrap_err();
        match err {
            RegistryError::HydrationFailed(msg) => {
                assert!(
                    msg.contains(abs.to_str().unwrap()) || msg.contains("does-not-exist"),
                    "hydration error should mention the missing path: {msg}"
                );
            }
            other => panic!("expected HydrationFailed, got {other:?}"),
        }
    }

    #[test]
    fn get_or_init_doc_hydrates_a_real_file_and_caches_it() {
        // Write a minimal sd.json to disk, register it, and hydrate.
        // Two consecutive calls should return Arcs that point at the
        // same underlying ProjectDoc (same allocation), demonstrating
        // the caching property.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let reg = ProjectRegistry::new(temp.path().to_path_buf());
        let abs = temp.path().join("model.sd.json");
        let json = r#"{"name":"demo","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        std::fs::write(&abs, json).expect("write file");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson),
        );

        let first = reg.get_or_init_doc(&abs).expect("first hydration");
        let second = reg.get_or_init_doc(&abs).expect("second hydration");
        assert!(
            Arc::ptr_eq(&first, &second),
            "second call must return the cached Arc, not re-hydrate"
        );
    }

    #[test]
    fn check_increment_and_merge_returns_not_found_for_unknown_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let err = reg
            .check_increment_and_merge(
                Path::new("/tmp/root/missing.stmx"),
                0,
                &serde_json::json!({}),
            )
            .unwrap_err();
        assert_eq!(err, RegistryError::NotFound);
    }

    #[test]
    fn check_increment_and_merge_rejects_stale_version() {
        // A stale expected_version must produce VersionMismatch and
        // leave both the registry version and the doc untouched.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let reg = ProjectRegistry::new(temp.path().to_path_buf());
        let abs = temp.path().join("m.sd.json");
        let initial = r#"{"name":"demo","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        std::fs::write(&abs, initial).expect("write");
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        meta.version = 5;
        reg.upsert(abs.clone(), meta);

        let err = reg
            .check_increment_and_merge(&abs, 4, &serde_json::json!({"name":"x"}))
            .unwrap_err();
        assert_eq!(
            err,
            RegistryError::VersionMismatch {
                expected: 4,
                actual: 5
            }
        );
        assert_eq!(reg.get(&abs).expect("entry").version, 5);
    }

    #[test]
    fn check_increment_and_merge_increments_and_applies_merge() {
        // Successful path: version increments and the merged JSON is
        // observable via the returned Arc<ProjectDoc>'s export.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let reg = ProjectRegistry::new(temp.path().to_path_buf());
        let abs = temp.path().join("m.sd.json");
        let initial = r#"{"name":"demo","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        std::fs::write(&abs, initial).expect("write");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson),
        );

        let new_json = serde_json::json!({
            "name":"renamed",
            "simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},
            "models":[{"name":"main"}]
        });
        let (version, doc) = reg
            .check_increment_and_merge(&abs, 0, &new_json)
            .expect("merge");
        assert_eq!(version, 1);
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported["name"].as_str(), Some("renamed"));
        assert_eq!(reg.get(&abs).expect("entry").version, 1);
    }

    #[test]
    fn merge_disk_change_returns_not_found_for_unknown_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let err = reg
            .merge_disk_change(Path::new("/tmp/root/missing.stmx"), &serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err, RegistryError::NotFound);
    }

    #[test]
    fn merge_disk_change_increments_version_without_expected_check() {
        // Watcher is authoritative for "what happened on disk", so unlike
        // check_increment_and_merge there is no expected_version argument.
        // Each call increments the version and applies the merge.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let reg = ProjectRegistry::new(temp.path().to_path_buf());
        let abs = temp.path().join("m.sd.json");
        let initial = r#"{"name":"on-disk","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        std::fs::write(&abs, initial).expect("write");
        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson);
        meta.version = 5;
        reg.upsert(abs.clone(), meta);

        let new_json = serde_json::json!({
            "name":"after-disk-edit",
            "simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},
            "models":[{"name":"main"}]
        });
        let new_version = reg.merge_disk_change(&abs, &new_json).expect("merge");
        assert_eq!(new_version, 6, "version increments without expected check");
        let entry = reg.get(&abs).expect("entry");
        assert_eq!(entry.version, 6);

        // Inspect the merged doc to verify the merge actually applied.
        let doc = reg.get_or_init_doc(&abs).expect("doc");
        let exported = doc.export_canonical_json().expect("export");
        assert_eq!(exported["name"].as_str(), Some("after-disk-edit"));
    }

    #[test]
    fn merge_disk_change_hydrates_from_disk_on_first_touch() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let reg = ProjectRegistry::new(temp.path().to_path_buf());
        let abs = temp.path().join("m.sd.json");
        let initial = r#"{"name":"baseline","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        std::fs::write(&abs, initial).expect("write");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson),
        );

        // doc slot is empty before the call.
        {
            let entry = reg.get(&abs).expect("entry");
            assert!(entry.doc.read().expect("read doc slot").is_none());
        }

        let updated = serde_json::json!({
            "name":"new-from-disk",
            "simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},
            "models":[{"name":"main"}]
        });
        let new_version = reg.merge_disk_change(&abs, &updated).expect("merge");
        assert_eq!(new_version, 1, "version starts at 0 + 1 = 1");

        let cached = reg.get_or_init_doc(&abs).expect("cached doc");
        let exported = cached.export_canonical_json().expect("export");
        assert_eq!(exported["name"].as_str(), Some("new-from-disk"));
    }

    #[test]
    fn rename_entry_returns_not_found_for_unknown_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let from = PathBuf::from("/tmp/root/missing.stmx");
        let to = PathBuf::from("/tmp/root/elsewhere.stmx");
        let err = reg.rename_entry(&from, &to).unwrap_err();
        assert_eq!(err, RegistryError::NotFound);
    }

    /// When the destination is already tracked, `rename_entry` must return
    /// `AlreadyExists` and leave both entries unchanged. The caller is
    /// responsible for emitting `ProjectRemoved` for the destination before
    /// deciding how to proceed, so the SPA never ends up with a stale
    /// sidebar entry that silently lost its state.
    #[test]
    fn rename_entry_returns_already_exists_when_destination_tracked() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let a = root.join("a.stmx");
        let b = root.join("b.stmx");

        let mut a_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        a_meta.version = 3;
        reg.upsert(a.clone(), a_meta);

        let mut b_meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        b_meta.version = 9;
        reg.upsert(b.clone(), b_meta);

        let err = reg.rename_entry(&a, &b).unwrap_err();
        assert_eq!(err, RegistryError::AlreadyExists);

        // Both entries must remain unchanged after the failed rename.
        assert_eq!(
            reg.get(&a).expect("a still present").version,
            3,
            "source entry must not be removed"
        );
        assert_eq!(
            reg.get(&b).expect("b still present").version,
            9,
            "destination entry must not be overwritten"
        );
    }

    #[test]
    fn rename_entry_re_keys_and_preserves_path_independent_state() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let from = root.join("a.stmx");
        let to = root.join("subdir").join("b.stmx");

        let mut meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx);
        meta.version = 7;
        meta.last_disk_hash = 0xfeed_face_dead_beefu64;
        let mut diag_keys = BTreeSet::new();
        diag_keys.insert(("syntax".to_string(), Some("x".to_string())));
        meta.last_diagnostic_keys = diag_keys.clone();
        reg.upsert(from.clone(), meta);

        let pre_doc_arc = reg.get(&from).expect("from exists").doc;

        reg.rename_entry(&from, &to).expect("rename succeeds");

        assert!(reg.get(&from).is_none(), "old key dropped");
        let entry = reg.get(&to).expect("new key present");
        assert_eq!(entry.version, 7, "version preserved");
        assert_eq!(
            entry.last_disk_hash, 0xfeed_face_dead_beefu64,
            "echo-suppression hash preserved"
        );
        assert_eq!(
            entry.last_diagnostic_keys, diag_keys,
            "diagnostic-key cache preserved"
        );
        assert_eq!(
            entry.path,
            PathBuf::from("subdir").join("b.stmx"),
            "display path is the new relative form"
        );
        assert!(
            Arc::ptr_eq(&pre_doc_arc, &entry.doc),
            "doc Arc carried over verbatim across re-key"
        );
    }

    #[test]
    fn rename_entry_keeps_format_from_pre_existing_meta() {
        // The destination's extension may differ from the source's
        // (.xmile -> .stmx). The registry stores the format the caller
        // already knows about; re-keying does not re-classify by
        // extension. (Callers that want to switch the format should
        // upsert the new meta directly.)
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let from = root.join("model.xmile");
        let to = root.join("model.stmx");

        let meta = make_meta(PathBuf::from("ignored"), ProjectFormat::Xmile);
        reg.upsert(from.clone(), meta);

        reg.rename_entry(&from, &to).expect("rename succeeds");
        let entry = reg.get(&to).expect("new key present");
        assert_eq!(
            entry.format,
            ProjectFormat::Xmile,
            "rename_entry preserves the recorded format"
        );
    }

    #[test]
    fn check_increment_and_merge_hydrates_from_disk_on_first_touch() {
        // Entry exists but the doc slot is empty. The merge call should
        // hydrate from disk inline (under the registry lock), apply the
        // new state, and return the hydrated+merged doc.
        let temp = tempfile::TempDir::new().expect("tempdir");
        let reg = ProjectRegistry::new(temp.path().to_path_buf());
        let abs = temp.path().join("m.sd.json");
        let initial = r#"{"name":"on-disk","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        std::fs::write(&abs, initial).expect("write");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::SdJson),
        );

        // doc slot is empty before the call.
        {
            let entry = reg.get(&abs).expect("entry");
            assert!(entry.doc.read().expect("read doc slot").is_none());
        }

        let updated = serde_json::json!({
            "name":"after-merge",
            "simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},
            "models":[{"name":"main"}]
        });
        let (version, _doc) = reg
            .check_increment_and_merge(&abs, 0, &updated)
            .expect("merge after hydration");
        assert_eq!(version, 1);

        // Subsequent get_or_init_doc returns the doc with the merged
        // state (proves caching took effect).
        let cached = reg.get_or_init_doc(&abs).expect("cached doc");
        let exported = cached.export_canonical_json().expect("export");
        assert_eq!(exported["name"].as_str(), Some("after-merge"));
    }
}
