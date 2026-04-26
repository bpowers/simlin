// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compose discovery + git probing into registry population.
//!
//! Phase 1 calls `scan_into_registry` once at startup and again on every
//! `GET /api/projects` so the registry is always fresh with respect to the
//! filesystem. Phase 4 introduces a watcher that drives incremental updates;
//! the surface here doesn't change.
//!
//! Per-file errors (missing metadata, unreadable file, transient git
//! failure) are logged at warn level and skipped rather than propagated:
//! one bad file shouldn't poison the whole listing.

use std::path::{Path, PathBuf};

use crate::discovery::{DiscoveryError, discover_models};
use crate::git::GitProbe;
use crate::registry::{ProjectMeta, ProjectRegistry};

/// Top-level scan failures. Per-file failures are *not* surfaced as errors;
/// they're logged and the file is skipped. `ScanError::Discovery` only fires
/// when the walker itself can't start (root missing, etc.).
#[derive(Debug)]
pub enum ScanError {
    Discovery(DiscoveryError),
    /// The configured root could not be canonicalized. We canonicalize so
    /// registry keys are unambiguous; if that fails we surface it rather
    /// than silently store non-canonical keys that won't match later
    /// canonicalized lookups.
    Root(PathBuf, std::io::Error),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Discovery(e) => write!(f, "{e}"),
            ScanError::Root(p, e) => {
                write!(f, "could not canonicalize root {}: {}", p.display(), e)
            }
        }
    }
}

impl std::error::Error for ScanError {}

/// Walk `root`, probe git for each match, and upsert a `ProjectMeta` into
/// `registry`. Returns the number of successful inserts.
///
/// `root` is canonicalized once up-front so registry keys are absolute and
/// stable; canonicalize the same way at lookup time. If the canonicalize
/// fails, we surface the error (vs. silently sharing keys that won't match).
///
/// After processing all discovered files, any registry entries whose
/// canonical path was *not* visited during this scan are removed. This
/// prevents stale "ghost" entries from accumulating when files are deleted
/// between scans. Phase 4's file watcher will replace this removal pass
/// with incremental add/remove events so listings never drift.
pub fn scan_into_registry(
    root: &Path,
    registry: &ProjectRegistry,
    git: &GitProbe,
) -> Result<usize, ScanError> {
    let canonical_root = root
        .canonicalize()
        .map_err(|e| ScanError::Root(root.to_path_buf(), e))?;

    let discovered = discover_models(&canonical_root).map_err(ScanError::Discovery)?;

    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut inserted = 0usize;
    for file in discovered {
        // Canonicalize so symlink-shadowed files dedupe with their real
        // targets in the registry. If canonicalize fails, fall back to the
        // raw absolute path so the file isn't silently lost.
        let abs_path = file
            .absolute_path
            .canonicalize()
            .unwrap_or_else(|_| file.absolute_path.clone());

        let meta_result = std::fs::metadata(&abs_path);
        let metadata = match meta_result {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    path = %abs_path.display(),
                    error = %e,
                    "skipping file: could not read metadata"
                );
                continue;
            }
        };

        let mtime = metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

        let git_state = git.status_for(&abs_path);

        let meta = ProjectMeta {
            // The registry's upsert overwrites this with the relativized
            // form; we set a placeholder here so the type-checker is happy.
            path: PathBuf::new(),
            format: file.format,
            mtime,
            size: metadata.len(),
            git: git_state,
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
        };

        visited.insert(abs_path.clone());
        registry.upsert_preserve_version(abs_path, meta);
        inserted += 1;
    }

    // Drop any registry entries that were not discovered this scan. These
    // are files that existed on a previous scan but have since been deleted.
    let stale: Vec<PathBuf> = registry
        .snapshot()
        .into_iter()
        .filter_map(|meta| {
            // snapshot() returns relative paths; we need the absolute key.
            let abs = canonical_root.join(&meta.path);
            if !visited.contains(&abs) {
                Some(abs)
            } else {
                None
            }
        })
        .collect();
    for path in stale {
        registry.remove(&path);
    }

    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{GitState, ProjectFormat};
    use std::fs;
    use tempfile::TempDir;

    fn touch(dir: &Path, rel: &str, contents: &[u8]) -> PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(&p, contents).expect("write file");
        p
    }

    #[test]
    fn scan_populates_registry_with_each_format() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "a.stmx", b"<root/>\n");
        touch(dir.path(), "b.xmile", b"<root/>\n");
        touch(dir.path(), "sub/c.mdl", b"contents");
        touch(dir.path(), "d.sd.json", b"{}");
        touch(dir.path(), "ignore-me.txt", b"unrelated");

        let canonical = dir.path().canonicalize().unwrap();
        let registry = ProjectRegistry::new(canonical.clone());
        let git = GitProbe::unavailable_for_tests();

        let inserted = scan_into_registry(dir.path(), &registry, &git).unwrap();
        assert_eq!(inserted, 4);
        assert_eq!(registry.len(), 4);

        let snap = registry.snapshot();
        let formats: Vec<ProjectFormat> = snap.iter().map(|m| m.format).collect();
        assert!(formats.contains(&ProjectFormat::Stmx));
        assert!(formats.contains(&ProjectFormat::Xmile));
        assert!(formats.contains(&ProjectFormat::Mdl));
        assert!(formats.contains(&ProjectFormat::SdJson));

        // With unavailable_for_tests every file should report Unavailable.
        for entry in &snap {
            assert_eq!(entry.git, GitState::Unavailable);
        }
    }

    #[test]
    fn scan_records_size_and_mtime_from_metadata() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "model.stmx", b"hello world");

        let canonical = dir.path().canonicalize().unwrap();
        let registry = ProjectRegistry::new(canonical.clone());
        let git = GitProbe::unavailable_for_tests();

        scan_into_registry(dir.path(), &registry, &git).unwrap();

        let entry = registry.snapshot().pop().expect("one entry");
        assert_eq!(entry.size, b"hello world".len() as u64);
        assert_eq!(entry.path, PathBuf::from("model.stmx"));
        assert!(entry.mtime <= std::time::SystemTime::now());
    }

    #[test]
    fn scan_returns_zero_when_no_models_present() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "readme.md", b"nothing of note");
        touch(dir.path(), "package.json", b"{}");

        let canonical = dir.path().canonicalize().unwrap();
        let registry = ProjectRegistry::new(canonical.clone());
        let git = GitProbe::unavailable_for_tests();

        let inserted = scan_into_registry(dir.path(), &registry, &git).unwrap();
        assert_eq!(inserted, 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn scan_with_missing_root_returns_root_error() {
        let canonical = Path::new("/this/should/not/exist/scanroot");
        let registry = ProjectRegistry::new(PathBuf::from("/tmp"));
        let git = GitProbe::unavailable_for_tests();

        let err = scan_into_registry(canonical, &registry, &git).unwrap_err();
        assert!(matches!(err, ScanError::Root(_, _)));
    }

    #[test]
    fn rescan_overwrites_existing_entry() {
        let dir = TempDir::new().unwrap();
        let path = touch(dir.path(), "model.stmx", b"v1");

        let canonical = dir.path().canonicalize().unwrap();
        let registry = ProjectRegistry::new(canonical.clone());
        let git = GitProbe::unavailable_for_tests();

        scan_into_registry(dir.path(), &registry, &git).unwrap();
        let first = registry.snapshot().pop().unwrap();
        assert_eq!(first.size, 2);

        fs::write(&path, b"version-two-payload").unwrap();
        scan_into_registry(dir.path(), &registry, &git).unwrap();
        assert_eq!(registry.len(), 1, "rescan should not duplicate the entry");
        let second = registry.snapshot().pop().unwrap();
        assert_eq!(second.size, b"version-two-payload".len() as u64);
    }

    #[test]
    fn rescan_preserves_version_after_check_and_increment() {
        // Regression: scan_into_registry must not reset a non-zero version
        // to 0. A client that saved (version 0 -> 1) and then triggers a
        // listing rescan must still get a 409 when retrying with version 0.
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "model.stmx", b"<root/>");

        let canonical = dir.path().canonicalize().unwrap();
        let registry = ProjectRegistry::new(canonical.clone());
        let git = GitProbe::unavailable_for_tests();

        // Initial scan: version is 0.
        scan_into_registry(dir.path(), &registry, &git).unwrap();
        let abs = canonical.join("model.stmx");
        assert_eq!(registry.get(&abs).unwrap().version, 0);

        // Simulate a successful save: version 0 -> 1.
        registry.check_and_increment(&abs, 0).unwrap();
        assert_eq!(registry.get(&abs).unwrap().version, 1);

        // Rescan (as triggered by GET /api/projects): version must stay 1.
        scan_into_registry(dir.path(), &registry, &git).unwrap();
        assert_eq!(
            registry.get(&abs).unwrap().version,
            1,
            "rescan must not reset version to 0"
        );
    }

    #[test]
    fn rescan_removes_deleted_file_from_registry() {
        // After a file is deleted between scans, the next scan must drop
        // the stale registry entry so it doesn't appear in listings.
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "a.stmx", b"<root/>");
        touch(dir.path(), "b.stmx", b"<root/>");

        let canonical = dir.path().canonicalize().unwrap();
        let registry = ProjectRegistry::new(canonical.clone());
        let git = GitProbe::unavailable_for_tests();

        scan_into_registry(dir.path(), &registry, &git).unwrap();
        assert_eq!(registry.len(), 2);

        // Delete file b; next scan must remove it from the registry.
        fs::remove_file(canonical.join("b.stmx")).unwrap();
        scan_into_registry(dir.path(), &registry, &git).unwrap();

        assert_eq!(
            registry.len(),
            1,
            "deleted file must be removed from registry"
        );
        let snap = registry.snapshot();
        assert_eq!(snap[0].path, PathBuf::from("a.stmx"));
    }
}
