// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-file git status detection via shell-out to the system `git` binary.
//!
//! We do not use libgit2 / gix because (a) we want to honor the user's
//! `core.hooksPath`, custom config, and submodule layout exactly the way
//! their installed `git` does, and (b) shelling out keeps our dependency
//! surface small. The cost is one or two extra processes per scan; for a
//! single-user local server that's well under the latency budget.
//!
//! Each `GitProbe` lazily caches `(porcelain output, ls-files output)` per
//! `(repo_root, mtime_of_index)`. When the index mtime changes — e.g. after
//! a commit, stage, or checkout — the next `status_for` call recomputes
//! transparently. Phase 4 will add explicit invalidation via the file
//! watcher (per AC2.4).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use crate::registry::GitState;

/// Cached porcelain + tracked-file results for one git working tree.
#[derive(Debug, Clone)]
struct RepoCache {
    /// Mtime of `<repo_root>/.git/index` at the time the cache was filled.
    /// `None` means "no index file existed when we last looked" — fresh
    /// repos with no commits hit this case and we treat the cache as
    /// always-stale.
    ///
    /// Race-window note: `build_repo_cache` runs the git commands first and
    /// reads this mtime afterward. If the index is rewritten between those two
    /// reads, the cache stores fresh-looking mtime metadata alongside
    /// slightly-stale data. The next request hits this cache entry and returns
    /// the stale data; only a *subsequent* index rewrite triggers
    /// recomputation. The window is bounded (one stale response in the worst
    /// case) and Phase 4's filesystem watcher closes it entirely (AC2.4).
    index_mtime: Option<SystemTime>,
    /// Map from absolute path to "is dirty" (true => `Tracked { dirty: true }`).
    /// Files not in this map but in `tracked` are `Tracked { dirty: false }`.
    dirty: HashMap<PathBuf, bool>,
    /// All paths reported by `git ls-files`, absolute.
    tracked: HashSet<PathBuf>,
}

/// Probes the local filesystem for git working trees and reports per-file
/// status. The probe is cheap to construct (`detect` runs `git --version`
/// once) and cheap to clone (the cache is `Arc`-shared).
#[derive(Debug, Clone)]
pub struct GitProbe {
    git_available: bool,
    cache: Arc<RwLock<HashMap<PathBuf, RepoCache>>>,
}

impl GitProbe {
    /// Probe the environment for a working `git` binary. We accept any
    /// non-zero exit as "git is broken or absent" because some shrink-wrapped
    /// containers ship a `git` shim that prints to stderr and exits 1; a
    /// negative result here just means we degrade to `Unavailable` for every
    /// file, which is the documented graceful-degradation path (AC2.5).
    pub fn detect() -> Self {
        let git_available = Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        Self {
            git_available,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Construct a probe that pretends git is unavailable. Only called from
    /// `test_support::unavailable_git_probe` and from unit tests inside this
    /// crate; never from production paths.
    pub(crate) fn new_unavailable() -> Self {
        Self {
            git_available: false,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// True when `git --version` succeeded at construction time. The SPA
    /// reads this to drive the AC2.5 "git unavailable" hint.
    pub fn git_available(&self) -> bool {
        self.git_available
    }

    /// Compute the VCS state for `path`. Caches per-repo results keyed on
    /// the index mtime so subsequent calls within the same scan are O(1).
    pub fn status_for(&self, path: &Path) -> GitState {
        if !self.git_available {
            return GitState::Unavailable;
        }

        let repo_root = match enclosing_git_root(path) {
            Some(root) => root,
            None => return GitState::Untracked,
        };

        let abs_path = path.to_path_buf();
        let current_mtime = index_mtime(&repo_root);

        if let Some(state) = self.lookup_cached(&repo_root, current_mtime, &abs_path) {
            return state;
        }

        let cache = match build_repo_cache(&repo_root) {
            Ok(c) => c,
            // If git itself fails (corrupt repo, permission error), fall
            // back to "we don't know" rather than crashing the request.
            Err(_) => return GitState::Unavailable,
        };

        let state = classify(&cache, &abs_path);
        self.store_cache(repo_root, cache);
        state
    }

    fn lookup_cached(
        &self,
        repo_root: &Path,
        current_mtime: Option<SystemTime>,
        abs_path: &Path,
    ) -> Option<GitState> {
        let guard = self.cache.read().expect("cache RwLock poisoned");
        let cache = guard.get(repo_root)?;
        if cache.index_mtime != current_mtime {
            return None;
        }
        Some(classify(cache, abs_path))
    }

    fn store_cache(&self, repo_root: PathBuf, cache: RepoCache) {
        let mut guard = self.cache.write().expect("cache RwLock poisoned");
        guard.insert(repo_root, cache);
    }

    /// Drop any cached `RepoCache` for `repo_root`. The next `status_for`
    /// call against any path inside that repo will rebuild the cache from
    /// fresh git output. Used by Phase 4's file watcher to close the
    /// race window where a `.git/index` mtime hasn't budged but the
    /// underlying status has changed (e.g., a `git reset` that rewinds
    /// to the pre-edit state and rewrites the index in-place with the
    /// same mtime).
    ///
    /// No-op when no entry exists for `repo_root`. Cheap to call: one
    /// `RwLock` write acquire + a `HashMap::remove`.
    pub fn invalidate_repo_cache(&self, repo_root: &Path) {
        let mut guard = self.cache.write().expect("cache RwLock poisoned");
        guard.remove(repo_root);
    }
}

/// Walk upward from `file.parent()` looking for the first ancestor that
/// contains a `.git` entry (directory in normal repos, regular file in
/// linked worktrees). Returns the working-tree root, not the gitdir.
pub fn enclosing_git_root(file: &Path) -> Option<PathBuf> {
    let mut current = if file.is_dir() {
        Some(file.to_path_buf())
    } else {
        file.parent().map(Path::to_path_buf)
    };
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// Read `<repo_root>/.git/index`'s mtime. Returns `None` when the index
/// doesn't exist (fresh repo with no staged content) — treating the cache as
/// always-stale in that window is fine because porcelain output is empty
/// then anyway.
fn index_mtime(repo_root: &Path) -> Option<SystemTime> {
    let dot_git = repo_root.join(".git");
    let index_path = if dot_git.is_dir() {
        dot_git.join("index")
    } else {
        // `.git` may be a regular file pointing into the parent worktree
        // (linked worktree layout). For Phase 1 we don't try to follow that
        // pointer; returning `None` keeps the cache stale and we just pay
        // the porcelain cost on every call. Phase 4's file watcher will
        // make this a non-issue.
        return None;
    };
    std::fs::metadata(&index_path)
        .and_then(|m| m.modified())
        .ok()
}

/// Run `git status --porcelain --untracked-files=all` + `git ls-files` and
/// build the per-repo cache. Errors propagate to the caller, which downgrades
/// the result to `Unavailable` (better to admit ignorance than to lie).
///
/// We pass `-c core.quotePath=false` to both commands so that paths containing
/// non-ASCII characters (e.g. `réservoir.stmx`) are emitted as raw UTF-8
/// rather than C-escaped octal sequences. Without this flag, git's default
/// `core.quotePath=true` would quote such filenames and the path strings would
/// not match the raw-byte paths from `ls-files` or our internal registry.
fn build_repo_cache(repo_root: &Path) -> std::io::Result<RepoCache> {
    let porcelain = run_git(
        repo_root,
        &[
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ],
    )?;
    let ls_files = run_git(repo_root, &["-c", "core.quotePath=false", "ls-files"])?;

    let mut dirty: HashMap<PathBuf, bool> = HashMap::new();
    for line in porcelain.lines() {
        // Lines are "XY path" where XY is exactly two status chars. With
        // `??` (untracked) we treat the file as dirty per the design: it's
        // inside the tree but not yet committed, so it's not "clean" and
        // not "outside the repo" either.
        if line.len() < 4 {
            continue;
        }
        let rel = line[3..].trim_start();
        // Renames have an arrow separator: "old -> new". We track the
        // post-rename path, which is what users see in their working copy.
        let rel = if let Some(idx) = rel.find(" -> ") {
            &rel[idx + 4..]
        } else {
            rel
        };
        let abs = repo_root.join(rel);
        dirty.insert(abs, true);
    }

    let mut tracked: HashSet<PathBuf> = HashSet::new();
    for rel in ls_files.lines() {
        if rel.is_empty() {
            continue;
        }
        tracked.insert(repo_root.join(rel));
    }

    Ok(RepoCache {
        index_mtime: index_mtime(repo_root),
        dirty,
        tracked,
    })
}

fn run_git(cwd: &Path, args: &[&str]) -> std::io::Result<String> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "git {} exited with {:?}: {}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Classify `abs_path` against a populated `RepoCache`. We canonicalize both
/// sides via `Path::components` comparison only when an exact match fails:
/// that handles trailing-slash and `./`-prefix discrepancies between the
/// query path and what `git ls-files` reported.
fn classify(cache: &RepoCache, abs_path: &Path) -> GitState {
    if cache.dirty.contains_key(abs_path) {
        return GitState::Tracked { dirty: true };
    }
    if cache.tracked.contains(abs_path) {
        return GitState::Tracked { dirty: false };
    }
    // Try canonical match for paths that survive symlinks or extra
    // separators. We only invoke `canonicalize` here (not above) because
    // it's the slow path; the fast path is exact-string equality.
    if let Ok(canonical) = abs_path.canonicalize() {
        if cache.dirty.contains_key(&canonical) {
            return GitState::Tracked { dirty: true };
        }
        if cache.tracked.contains(&canonical) {
            return GitState::Tracked { dirty: false };
        }
    }
    GitState::Untracked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Run `git` in `cwd` with `args`. Asserts success; returns stdout as String.
    fn must_git(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
        if !out.status.success() {
            panic!(
                "git {} exited {:?}: {}",
                args.join(" "),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Check whether `git` is available on the host. Tests that exercise
    /// real git workflows skip when this returns false (CI runs with git
    /// installed, but in case a sandbox doesn't have it, we degrade
    /// gracefully rather than fail).
    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn unavailable_probe_returns_unavailable_for_every_path() {
        let probe = GitProbe::new_unavailable();
        assert!(!probe.git_available());
        assert_eq!(
            probe.status_for(Path::new("/tmp/missing")),
            GitState::Unavailable
        );
        assert_eq!(probe.status_for(Path::new("/")), GitState::Unavailable);
    }

    #[test]
    fn enclosing_git_root_returns_none_for_path_outside_any_repo() {
        // /tmp itself should not have a .git directory in normal CI.
        // We don't assert against /tmp directly (someone could have a repo
        // there) but we test a path component the test owns.
        let dir = tempfile::TempDir::new().unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        // The tempdir lives under the system tmp; it is *possible* for a
        // user to have a .git anywhere upstream. We accept whatever the
        // system reports — the contract is just "Some -> a real .git
        // exists at that root".
        let result = enclosing_git_root(&nested.join("c.stmx"));
        if let Some(root) = result {
            assert!(root.join(".git").exists());
        }
    }

    #[test]
    fn invalidate_repo_cache_removes_cached_entries_for_that_repo() {
        if !git_available() {
            eprintln!("skipping: git binary not available");
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        must_git(repo, &["init", "-q"]);
        must_git(repo, &["config", "user.email", "test@example.com"]);
        must_git(repo, &["config", "user.name", "test"]);
        let model = repo.join("model.stmx");
        std::fs::write(&model, b"<root/>").unwrap();
        must_git(repo, &["add", "model.stmx"]);
        must_git(repo, &["commit", "-q", "-m", "initial"]);

        let probe = GitProbe::detect();
        // First call populates the cache.
        let _ = probe.status_for(&model);
        // Cache must now contain an entry for this repo.
        assert!(
            probe.cache.read().unwrap().contains_key(repo),
            "cache should hold a RepoCache after status_for"
        );

        probe.invalidate_repo_cache(repo);
        assert!(
            !probe.cache.read().unwrap().contains_key(repo),
            "invalidate_repo_cache must remove the entry"
        );
    }

    #[test]
    fn invalidate_repo_cache_is_noop_for_unknown_repo() {
        let probe = GitProbe::new_unavailable();
        // No panic, no error; just a quiet no-op.
        probe.invalidate_repo_cache(Path::new("/nope/not/a/real/repo"));
    }

    #[test]
    fn enclosing_git_root_finds_marker_at_each_level() {
        let dir = tempfile::TempDir::new().unwrap();
        let nested = dir.path().join("sub").join("deeper");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();

        let from_file = nested.join("model.stmx");
        std::fs::write(&from_file, "").unwrap();

        let found = enclosing_git_root(&from_file).expect("repo root");
        assert_eq!(found, dir.path());
    }
}
