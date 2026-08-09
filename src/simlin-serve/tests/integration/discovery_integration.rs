// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end discovery tests against real tempdir layouts. These cover the
//! ACs from the design doc that exercise the walker as a black box: which
//! files end up in the result set for a given on-disk shape. Per-extension
//! format mapping is covered by the unit tests in `src/discovery.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use simlin_serve::discovery::discover_models;
use tempfile::TempDir;

fn write_file(dir: &Path, rel: &str, contents: &str) -> PathBuf {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(&p, contents).expect("write file");
    p
}

#[test]
fn ac1_2_recurses_into_subdirectories() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "sub/d.stmx", "");
    write_file(dir.path(), "deeper/again/e.xmile", "");

    let found = discover_models(dir.path()).unwrap();
    assert_eq!(found.len(), 2);

    let suffixes: Vec<String> = found
        .iter()
        .map(|f| {
            f.absolute_path
                .strip_prefix(dir.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert!(
        suffixes.iter().any(|s| s == "sub/d.stmx"),
        "expected sub/d.stmx in {:?}",
        suffixes
    );
    assert!(
        suffixes.iter().any(|s| s == "deeper/again/e.xmile"),
        "expected deeper/again/e.xmile in {:?}",
        suffixes
    );
}

#[test]
fn ac1_3_excludes_universal_generated_directories() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "node_modules/x.stmx", "");
    write_file(dir.path(), ".git/x.stmx", "");
    write_file(dir.path(), "target/x.stmx", "");
    write_file(dir.path(), "playwright-report/x.stmx", "");
    write_file(dir.path(), "test-results/x.stmx", "");
    let visible = write_file(dir.path(), "kept.stmx", "");

    let found = discover_models(dir.path()).unwrap();
    assert_eq!(found.len(), 1, "only kept.stmx should be discovered");
    assert_eq!(found[0].absolute_path, visible);
}

#[cfg(unix)]
#[test]
fn ac1_5_does_not_follow_symlinked_directories() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let real = write_file(dir.path(), "real.stmx", "");

    // Self-referential cycle: walking into `loop` would re-enter the same
    // tempdir indefinitely if symlinks were followed.
    symlink(dir.path(), dir.path().join("loop")).expect("create symlink");

    let found = discover_models(dir.path()).unwrap();
    // We expect exactly one hit: the real file. With follow_links(false),
    // the symlinked directory is skipped, so we never see `real.stmx` again
    // through `loop/real.stmx`.
    let real_canonical = real.canonicalize().unwrap_or_else(|_| real.clone());
    let canonical_paths: Vec<PathBuf> = found
        .iter()
        .map(|f| {
            f.absolute_path
                .canonicalize()
                .unwrap_or_else(|_| f.absolute_path.clone())
        })
        .collect();
    let real_hits = canonical_paths
        .iter()
        .filter(|p| **p == real_canonical)
        .count();
    assert_eq!(
        real_hits, 1,
        "expected the real file to be discovered exactly once, got {} (paths: {:?})",
        real_hits, canonical_paths
    );
    // And no infinite recursion: the result count is finite (the test would
    // hang or OOM otherwise; this assertion documents the intent).
    assert!(
        found.len() < 100,
        "discovery returned {} files; symlink loop suspected",
        found.len()
    );
}
