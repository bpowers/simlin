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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use serde::Serialize;

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
}

/// Failures produced by registry operations that need to report a
/// distinguishable error rather than just succeed-or-do-nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// No entry exists for the given absolute path.
    NotFound,
    /// The caller's `expected_version` did not match the entry's stored
    /// version. `actual` is the current value as observed under the lock so
    /// the caller can refetch against it.
    VersionMismatch { expected: u64, actual: u64 },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NotFound => write!(f, "registry entry not found"),
            RegistryError::VersionMismatch { expected, actual } => {
                write!(f, "version mismatch: expected {expected}, actual {actual}")
            }
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

    /// Update the entry's `mtime` to `mtime`. No-op if the path is not in
    /// the registry. Used by the save handler after a successful disk
    /// write so a subsequent listing reflects the new modification time.
    /// Task 7 extends this with size refresh; for Task 5 the mtime alone
    /// is enough to keep the SPA's stale-data heuristics in sync with
    /// disk reality.
    pub fn refresh_meta_mtime(&self, abs_path: &Path, mtime: SystemTime) {
        let mut guard = self
            .inner
            .write()
            .expect("registry RwLock poisoned by panic in another thread");
        if let Some(entry) = guard.get_mut(abs_path) {
            entry.mtime = mtime;
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
        let new_meta = ProjectMeta {
            path: relativize(&self.root, &sidecar_path),
            format: ProjectFormat::SdJson,
            mtime: prev.mtime,
            size: prev.size,
            git: prev.git,
            version: prev.version,
        };
        guard.insert(sidecar_path, new_meta);
        Ok(())
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
    fn refresh_meta_mtime_updates_existing_entry() {
        let root = PathBuf::from("/tmp/root");
        let reg = ProjectRegistry::new(root.clone());
        let abs = root.join("model.stmx");
        reg.upsert(
            abs.clone(),
            make_meta(PathBuf::from("ignored"), ProjectFormat::Stmx),
        );
        let updated_mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_500);
        reg.refresh_meta_mtime(&abs, updated_mtime);
        assert_eq!(reg.get(&abs).expect("entry").mtime, updated_mtime);
    }

    #[test]
    fn refresh_meta_mtime_is_noop_for_missing_path() {
        let reg = ProjectRegistry::new(PathBuf::from("/tmp/root"));
        let nonexistent = PathBuf::from("/tmp/root/missing.stmx");
        reg.refresh_meta_mtime(
            &nonexistent,
            SystemTime::UNIX_EPOCH + Duration::from_secs(42),
        );
        assert!(reg.is_empty());
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
}
