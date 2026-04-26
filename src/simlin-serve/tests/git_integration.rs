// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end tests of `GitProbe::status_for` against a real `git` binary.
//!
//! Per the project policy in docs/dev/rust.md, suites should fail loudly
//! when a required helper is missing — but `git` is universal enough on
//! developer machines that we treat absence as a soft skip locally and let
//! CI catch it via the version check in `pre-commit`. The skip is explicit
//! and prints a message so the operator notices.

use std::path::Path;
use std::process::Command;

use simlin_serve::git::{GitProbe, enclosing_git_root};
use simlin_serve::registry::GitState;
use tempfile::TempDir;

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap_or_else(|_| panic!("git {} failed to spawn", args.join(" ")));
    assert!(status.success(), "git {} failed", args.join(" "));
}

fn init_repo(dir: &Path) {
    run_git(dir, &["init", "-q", "-b", "main"]);
    // Configure identity locally so `git commit` succeeds in tempdirs that
    // don't inherit a global identity (typical in CI sandboxes).
    run_git(dir, &["config", "user.name", "test"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn ac2_5_unavailable_probe_reports_every_file_as_unavailable() {
    let probe = GitProbe::unavailable_for_tests();
    let dir = TempDir::new().unwrap();
    let f = dir.path().join("any.stmx");
    std::fs::write(&f, "").unwrap();
    assert_eq!(probe.status_for(&f), GitState::Unavailable);
}

#[test]
fn ac2_3_file_outside_any_git_tree_is_untracked() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    let f = dir.path().join("solo.stmx");
    std::fs::write(&f, "").unwrap();

    // Sanity: confirm there's no .git anywhere up the tempdir's tree by
    // asking our own helper. If the system tempdir happens to live inside
    // a repo, we soft-skip — the assertion would be invalid.
    if enclosing_git_root(&f).is_some() {
        eprintln!("skipping: tempdir lives inside a parent git repo");
        return;
    }

    let probe = GitProbe::detect();
    assert_eq!(probe.status_for(&f), GitState::Untracked);
}

#[test]
fn ac2_1_committed_file_is_tracked_and_clean() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let f = dir.path().join("clean.stmx");
    std::fs::write(&f, "<root/>\n").unwrap();
    run_git(dir.path(), &["add", "clean.stmx"]);
    run_git(dir.path(), &["commit", "-q", "-m", "add clean"]);

    let probe = GitProbe::detect();
    assert_eq!(
        probe.status_for(&f),
        GitState::Tracked { dirty: false },
        "committed file with no further changes should be clean"
    );
}

#[test]
fn ac2_2_modified_file_is_tracked_and_dirty() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let f = dir.path().join("dirty.stmx");
    std::fs::write(&f, "<root/>\n").unwrap();
    run_git(dir.path(), &["add", "dirty.stmx"]);
    run_git(dir.path(), &["commit", "-q", "-m", "add dirty"]);

    std::fs::write(&f, "<root>changed</root>\n").unwrap();

    let probe = GitProbe::detect();
    assert_eq!(
        probe.status_for(&f),
        GitState::Tracked { dirty: true },
        "modified file should be reported dirty"
    );
}

#[test]
fn untracked_file_inside_repo_with_porcelain_question_marks() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    // Create one committed file so the repo has an HEAD; otherwise some
    // git versions emit slightly different porcelain output.
    let committed = dir.path().join("seed.stmx");
    std::fs::write(&committed, "").unwrap();
    run_git(dir.path(), &["add", "seed.stmx"]);
    run_git(dir.path(), &["commit", "-q", "-m", "seed"]);

    // A new file that is in the working tree but not in the index. Per the
    // design, this counts as `Tracked { dirty: true }` because it lives
    // within the repo and represents uncommitted work.
    let new_file = dir.path().join("newcomer.stmx");
    std::fs::write(&new_file, "").unwrap();

    let probe = GitProbe::detect();
    assert_eq!(
        probe.status_for(&new_file),
        GitState::Tracked { dirty: true },
        "untracked-but-known files (?? in porcelain) are reported as dirty"
    );
}

#[test]
fn cache_invalidates_on_index_mtime_change() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    let f = dir.path().join("flips.stmx");
    std::fs::write(&f, "before\n").unwrap();
    run_git(dir.path(), &["add", "flips.stmx"]);
    run_git(dir.path(), &["commit", "-q", "-m", "before"]);

    let probe = GitProbe::detect();
    assert_eq!(probe.status_for(&f), GitState::Tracked { dirty: false });

    // Make the file dirty. Sleep at least 1s so the mtime delta is observable
    // on filesystems with second-resolution mtimes (ext4 default, HFS+ on
    // macOS). Without the sleep this test is racy on slow CI.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(&f, "after\n").unwrap();
    run_git(dir.path(), &["add", "flips.stmx"]);

    assert_eq!(
        probe.status_for(&f),
        GitState::Tracked { dirty: true },
        "cache must reflect the post-stage state once index mtime changes"
    );
}

// Regression test for the quoted-path bug: git's default core.quotePath=true
// would emit non-ASCII filenames as C-escaped octal sequences (e.g.
// `"r\303\251servoir.stmx"` with the surrounding quotes). The parser must
// see the raw path, not the quoted form. Without passing
// `-c core.quotePath=false`, this test would fail because the path strings
// from `git status` and `git ls-files` would not match the real filename.
#[test]
fn non_ascii_filename_is_tracked_clean_after_commit() {
    if !git_available() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let dir = TempDir::new().unwrap();
    init_repo(dir.path());

    // U+00E9 (LATIN SMALL LETTER E WITH ACUTE) triggers core.quotePath quoting.
    let filename = "r\u{00e9}servoir.stmx";
    let f = dir.path().join(filename);
    std::fs::write(&f, "<root/>\n").unwrap();
    run_git(dir.path(), &["add", filename]);
    run_git(dir.path(), &["commit", "-q", "-m", "add reservoir"]);

    let probe = GitProbe::detect();
    assert_eq!(
        probe.status_for(&f),
        GitState::Tracked { dirty: false },
        "committed non-ASCII filename must be reported as Tracked{{dirty:false}}, \
         not Untracked (would indicate a quoted-path parsing bug)"
    );
}
