// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Recursive filesystem discovery of system-dynamics model files.
//!
//! Built on top of the `ignore` crate so the user's `.gitignore` rules are
//! honored automatically — that covers project-specific build artifacts
//! (`lib/`, `build/`, `dist/`, etc.) without us hardcoding them. We add an
//! explicit denylist only for *universal* directories that have no business
//! being scanned regardless of `.gitignore` configuration: `node_modules`,
//! `.git`, `target`, `playwright-report`, `test-results`.
//!
//! `follow_links(false)` is left at the `ignore` crate default so symlink
//! cycles cannot loop the walker (per AC1.5).

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::registry::ProjectFormat;

/// One discovered file. The path is always absolute (the `ignore` crate
/// returns absolute paths when given an absolute root, but we pass through
/// whatever `WalkBuilder` produced — callers that need canonicalization must
/// run `fs::canonicalize` themselves).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub absolute_path: PathBuf,
    pub format: ProjectFormat,
}

/// Errors raised during the directory walk itself. Per-entry errors (e.g. a
/// single unreadable file inside an otherwise-readable tree) are *not*
/// surfaced here — the `ignore` crate skips them silently and we mirror that
/// behavior because partial discovery is more useful than a hard failure.
#[derive(Debug)]
pub enum DiscoveryError {
    /// The configured root does not exist or is not a directory.
    InvalidRoot(PathBuf),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryError::InvalidRoot(p) => {
                write!(
                    f,
                    "discovery root does not exist or is not a directory: {}",
                    p.display()
                )
            }
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Directory names we never descend into, regardless of `.gitignore` state.
/// Kept short on purpose: anything project-specific belongs in the user's
/// `.gitignore`, which the `ignore` crate honors automatically.
const UNIVERSAL_EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "playwright-report",
    "test-results",
];

/// Walk `root` recursively, returning every file whose extension maps to a
/// known [`ProjectFormat`]. Order is whatever the underlying walker
/// produces; callers needing determinism should sort.
pub fn discover_models(root: &Path) -> Result<Vec<DiscoveredFile>, DiscoveryError> {
    if !root.is_dir() {
        return Err(DiscoveryError::InvalidRoot(root.to_path_buf()));
    }

    let walker = WalkBuilder::new(root)
        .follow_links(false)
        .git_ignore(true)
        .git_global(true)
        .filter_entry(|entry| {
            // The filter applies to both files and directories. We only want
            // to *exclude directories whose names appear in the universal
            // denylist*; files always get through to the format-detection
            // step below.
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if !is_dir {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            !UNIVERSAL_EXCLUDED_DIRS.iter().any(|d| name == *d)
        })
        .build();

    let mut out = Vec::new();
    for result in walker {
        let entry = match result {
            Ok(e) => e,
            // Permission errors and the like are best-effort; one unreadable
            // entry shouldn't kill the whole scan.
            Err(_) => continue,
        };
        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
            && let Some(format) = format_for_path(entry.path())
        {
            out.push(DiscoveredFile {
                absolute_path: entry.path().to_path_buf(),
                format,
            });
        }
    }

    Ok(out)
}

/// Map a filesystem path to a `ProjectFormat` purely by extension. The
/// `.sd.json` discriminator is matched on the literal compound suffix because
/// the `Path::extension` API only returns the trailing component (`json`).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(dir: &Path, rel: &str, contents: &str) -> PathBuf {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(&p, contents).expect("write file");
        p
    }

    fn formats_for(found: &[DiscoveredFile]) -> Vec<(String, ProjectFormat)> {
        let mut out: Vec<(String, ProjectFormat)> = found
            .iter()
            .map(|f| {
                (
                    f.absolute_path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                    f.format,
                )
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn extension_dispatcher_recognizes_known_formats() {
        assert_eq!(
            format_for_path(Path::new("/tmp/a.stmx")),
            Some(ProjectFormat::Stmx)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/a.xmile")),
            Some(ProjectFormat::Xmile)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/a.xml")),
            Some(ProjectFormat::Xmile)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/a.mdl")),
            Some(ProjectFormat::Mdl)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/a.sd.json")),
            Some(ProjectFormat::SdJson)
        );
        assert_eq!(
            format_for_path(Path::new("/tmp/a.STMX")),
            Some(ProjectFormat::Stmx),
            "extension match is case-insensitive"
        );
        assert_eq!(format_for_path(Path::new("/tmp/a.json")), None);
        assert_eq!(format_for_path(Path::new("/tmp/a.txt")), None);
        assert_eq!(format_for_path(Path::new("/tmp/noext")), None);
    }

    #[test]
    fn invalid_root_returns_error() {
        let result = discover_models(Path::new("/this/path/does/not/exist/we-hope"));
        assert!(matches!(result, Err(DiscoveryError::InvalidRoot(_))));
    }

    #[test]
    fn discovers_three_known_formats_at_root() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "a.stmx", "");
        write_file(dir.path(), "b.xmile", "");
        write_file(dir.path(), "c.mdl", "");
        write_file(dir.path(), "ignore-me.txt", "");

        let found = discover_models(dir.path()).unwrap();
        assert_eq!(
            formats_for(&found),
            vec![
                ("a.stmx".into(), ProjectFormat::Stmx),
                ("b.xmile".into(), ProjectFormat::Xmile),
                ("c.mdl".into(), ProjectFormat::Mdl),
            ]
        );
    }

    #[test]
    fn discovers_sd_json_via_compound_suffix() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "model.sd.json", "{}");
        write_file(dir.path(), "package.json", "{}");

        let found = discover_models(dir.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].format, ProjectFormat::SdJson);
    }
}
