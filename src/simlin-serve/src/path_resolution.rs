// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core

//! Shared path-resolution primitives.
//!
//! Three concerns recur across the HTTP handlers, the MCP `RegistryAccess`
//! impl, the watcher, and the scanner:
//!
//! 1. **Reading an existing path safely** — the leaf must exist; canonicalize
//!    it and confirm it descends from the registry root. Symlinks within the
//!    tree are accepted, ones pointing out of the tree are rejected.
//! 2. **Writing to a path that does not yet exist** — canonicalize fails on
//!    missing leaves, so we walk up to the deepest *existing* ancestor and
//!    canonicalize that. The boundary check applies before any byte hits
//!    disk so a symlinked parent dir cannot create files outside the root.
//! 3. **Resolving the registry's canonical key** — for `.mdl` paths, when a
//!    sibling `.sd.json` exists on disk it becomes the canonical entry for
//!    both reads and writes. (See `RegistryKey` below.)
//!
//! Centralizing these here removes the class of bug "consumer X forgot to
//! apply the rule consumer Y enforces": the rules are implemented once and
//! every consumer calls the same function.

use std::path::{Path, PathBuf};

/// Error returned by [`resolve_create_target`]. Generic over the caller's
/// own error variant (`AccessError`, `SaveError`, etc.) because each
/// transport renders rejection differently.
///
/// Callers map [`Self::OutOfRoot`] to their authorization-failure variant
/// and [`Self::IoError`] to their internal-error variant. The carrier of
/// `IoError` is the underlying `std::io::Error` so the caller can preserve
/// the original `kind()` for downstream `match`es.
#[derive(Debug)]
pub enum CreatePathError {
    /// The resolved path escapes the canonicalized scan root, or contains a
    /// `..`/root/prefix segment that cannot be reasoned about lexically.
    OutOfRoot,
    /// `canonicalize` on the deepest existing ancestor (or on the root)
    /// failed for some reason other than non-existence — most commonly a
    /// permissions or I/O error.
    IoError(std::io::Error),
}

/// Resolve `abs_path` (which **does not yet exist** at its leaf) to a
/// canonical absolute path inside `root_canonical`, rejecting symlinked
/// or `..`-traversal escapes before the file is created.
///
/// The algorithm:
///
/// 1. Walk up `abs_path` until we find an existing ancestor (the leaf
///    cannot exist because we are about to create it).
/// 2. Canonicalize that ancestor — this resolves any symlinks in the
///    existing prefix.
/// 3. Confirm the canonical ancestor descends from `root_canonical`.
/// 4. Compose the resolved path: canonical ancestor + the remaining
///    lexical segments. We reject `..`, `RootDir`, and `Prefix` segments
///    in the remainder because the remainder was never part of the
///    canonicalized prefix and there is no filesystem-level reasoning
///    available for it.
/// 5. Final boundary check on the composed path.
///
/// Returns the resolved absolute path on success.
pub fn resolve_create_target(
    abs_path: &Path,
    root_canonical: &Path,
) -> Result<PathBuf, CreatePathError> {
    // Find the deepest existing ancestor.
    let mut existing_ancestor = abs_path;
    while !existing_ancestor.exists() {
        match existing_ancestor.parent() {
            Some(parent) => existing_ancestor = parent,
            None => return Err(CreatePathError::OutOfRoot),
        }
    }
    let canonical_ancestor = existing_ancestor
        .canonicalize()
        .map_err(CreatePathError::IoError)?;
    if !canonical_ancestor.starts_with(root_canonical) {
        return Err(CreatePathError::OutOfRoot);
    }

    // Compose the canonical prefix with the lexical remainder. The
    // remainder is anything past `existing_ancestor` in the requested
    // path; we walk its components and reject anything other than
    // `Normal` segments because the remainder was not part of the
    // filesystem-canonicalized prefix.
    let remainder = abs_path
        .strip_prefix(existing_ancestor)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| abs_path.to_path_buf());
    let mut resolved = canonical_ancestor;
    for component in remainder.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                resolved.push(name);
            }
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(CreatePathError::OutOfRoot);
            }
        }
    }

    if !resolved.starts_with(root_canonical) {
        return Err(CreatePathError::OutOfRoot);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn resolves_inside_root_when_parent_exists() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let target = root.join("brand_new.stmx");

        let resolved = resolve_create_target(&target, &root).expect("resolves");
        assert_eq!(resolved, target);
    }

    #[test]
    fn resolves_when_a_subdirectory_must_be_created() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let target = root.join("nested").join("dir").join("model.stmx");

        let resolved = resolve_create_target(&target, &root).expect("resolves");
        // The resolved path is the lexical composition: canonical root
        // + remainder.
        assert_eq!(resolved, target);
    }

    #[test]
    fn rejects_traversal_via_dotdot_segment() {
        let temp = TempDir::new().expect("tempdir");
        let outer = temp.path().canonicalize().expect("canon outer");
        let inner = outer.join("inner");
        fs::create_dir(&inner).expect("mkdir inner");
        let escape = inner.join("..").join("escape.stmx");

        match resolve_create_target(&escape, &inner) {
            Err(CreatePathError::OutOfRoot) => {}
            other => panic!("expected OutOfRoot, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_pointing_outside_root() {
        // Create an inner root and an outer destination. Inside the inner
        // root, `escape` is a symlink to the outer destination. A request
        // to create `<inner>/escape/foo.stmx` must be rejected before the
        // file lands at `<outer>/escape_target/foo.stmx`.
        let temp = TempDir::new().expect("tempdir");
        let outer = temp.path().canonicalize().expect("canon outer");
        let inner = outer.join("inner");
        let escape_target = outer.join("escape_target");
        fs::create_dir(&inner).expect("mkdir inner");
        fs::create_dir(&escape_target).expect("mkdir escape_target");

        let symlink = inner.join("escape");
        std::os::unix::fs::symlink(&escape_target, &symlink).expect("symlink");

        let target = inner.join("escape").join("model.stmx");
        match resolve_create_target(&target, &inner) {
            Err(CreatePathError::OutOfRoot) => {}
            other => panic!("expected OutOfRoot, got {other:?}"),
        }

        // Sanity check: we did not write anything (the function is pure).
        assert!(
            !escape_target.join("model.stmx").exists(),
            "resolve_create_target must not have created the file outside root"
        );
    }
}
