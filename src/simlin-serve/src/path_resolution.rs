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
//!    Implemented by [`resolve_existing_within_root`].
//! 2. **Writing to a path that does not yet exist** — canonicalize fails on
//!    missing leaves, so we walk up to the deepest *existing* ancestor and
//!    canonicalize that. The boundary check applies before any byte hits
//!    disk so a symlinked parent dir cannot create files outside the root.
//!    Implemented by [`resolve_canonical_path`] (also exposed as
//!    [`resolve_create_target`] for write-side call sites that prefer the
//!    pre-write naming).
//! 3. **Observing a path whose leaf just disappeared** — rename source
//!    paths and removal events arrive at the watcher *after* the leaf
//!    is gone, so plain `canonicalize()` would fail and plain
//!    `strip_prefix(root)` would miss when the OS watcher reports the
//!    path through an unresolved symlink alias of the root (notably on
//!    macOS, where `/var/folders/...` aliases `/private/var/folders/...`).
//!    The same algorithm as #2 — canonicalize the deepest existing
//!    ancestor and re-attach the lexical remainder — produces the
//!    registry's canonical key for both write-side and observe-side
//!    consumers, so they share [`resolve_canonical_path`].
//!
//! Every project file is its own registry entry keyed by its own canonical
//! path; there is no per-format aliasing (a `.mdl` is read from and written
//! to the `.mdl`), so no fourth rule maps one path onto another.
//!
//! Centralizing these here removes the class of bug "consumer X forgot to
//! apply the rule consumer Y enforces": the rules are implemented once and
//! every consumer calls the same function. The trivial helper
//! [`to_forward_slash`] lives here for the same reason — different
//! consumers had drifted on string-rendering, producing the same shape of
//! bug.

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Render a relative `Path` as a forward-slash string for the WebSocket /
/// HTTP wire format.
///
/// On Unix this is a no-op cast; on Windows it rewrites `\` to `/` so URL
/// segments work without further escaping. The conversion is lossy if the
/// path contains non-UTF-8 bytes — the resulting string substitutes the
/// Unicode replacement character — but that is the correct behaviour for
/// JSON payloads, which require well-formed UTF-8 by definition.
pub fn to_forward_slash(path: &Path) -> String {
    let display = path.to_string_lossy().into_owned();
    if MAIN_SEPARATOR == '/' {
        display
    } else {
        display.replace(MAIN_SEPARATOR, "/")
    }
}

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

/// Error returned by [`resolve_existing_within_root`]. Distinguishes the
/// three cases the consumers need to map differently:
///
/// - `NotFound` — the path's leaf does not exist on disk; HTTP renders 404,
///   `SaveError` renders 404, MCP collapses (along with the other variants)
///   to `AccessError::NotFound`.
/// - `OutOfRoot` — the canonicalized leaf is not a descendant of the
///   canonicalized root; HTTP renders 403, `SaveError` renders 403, MCP
///   collapses to `AccessError::NotFound` so it does not leak filesystem
///   layout.
/// - `IoError` — any other failure of `canonicalize()` (typically a
///   permissions error on an intermediate directory or on the root itself);
///   HTTP / `SaveError` render 500, MCP again collapses to `NotFound`.
///
/// The variant boundary intentionally puts root-canonicalize errors in the
/// same `IoError` bucket as leaf-canonicalize errors that aren't `NotFound`:
/// every consumer treats them the same way (500 Internal in HTTP, NotFound
/// in MCP), and downstream callers that wanted to differentiate the
/// underlying source can do so by looking at the carried `std::io::Error`'s
/// `raw_os_error()` or `kind()`.
#[derive(Debug)]
pub enum ResolutionError {
    /// `abs_path` does not exist on disk.
    NotFound,
    /// The canonicalized path is not a descendant of `root_canonical`.
    OutOfRoot,
    /// `canonicalize()` on either the path or the root itself failed for a
    /// reason other than non-existence (e.g. EACCES on a parent dir).
    IoError(std::io::Error),
}

/// Canonicalize `abs_path` and confirm it descends from `root_canonical`.
///
/// Used by every read-path consumer (HTTP `get_project`, HTTP
/// `save_project`, MCP `open`, MCP `save`, the create handler's
/// post-write validation) to enforce the "leaf is inside the registry
/// root" invariant uniformly. Each transport applies its own mapping to
/// the returned [`ResolutionError`]:
///
/// - HTTP `get_project` / `save_project` distinguish 404 / 403 / 500.
/// - `RegistryAccess` collapses every variant to `AccessError::NotFound`
///   so MCP clients cannot probe for files they don't have permission to
///   read.
/// - The create handler's post-write check sees `NotFound` /
///   `IoError` only when the freshly-written file races with another
///   process; it surfaces them as 500 and `OutOfRoot` as 403.
///
/// Returns the canonicalized path on success.
pub fn resolve_existing_within_root(
    abs_path: &Path,
    root_canonical: &Path,
) -> Result<PathBuf, ResolutionError> {
    let canonical = match abs_path.canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ResolutionError::NotFound);
        }
        Err(e) => return Err(ResolutionError::IoError(e)),
    };
    if !canonical.starts_with(root_canonical) {
        return Err(ResolutionError::OutOfRoot);
    }
    Ok(canonical)
}

/// Resolve `abs_path` to a canonical absolute path inside
/// `root_canonical`, rejecting symlinked or `..`-traversal escapes.
/// Works whether or not `abs_path`'s leaf currently exists on disk.
///
/// The algorithm:
///
/// 1. Walk up `abs_path` until we find an existing ancestor. When the
///    leaf exists, the first iteration succeeds and we canonicalize
///    the leaf itself; when it does not (yet-to-be-created files,
///    rename sources, removed files), we descend through ancestors
///    until something exists.
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
/// Used by both write-side and observe-side consumers:
///
/// - Pre-write (HTTP `create_new_project`, MCP create flow,
///   `save_project` post-write check): see [`resolve_create_target`],
///   which is a thin alias documenting the create-target intent.
/// - Post-disappearance (watcher `handle_model_removal`, the source
///   side of `handle_model_rename`): the leaf has just been unlinked,
///   so a plain `canonicalize()` on the leaf would error and a plain
///   `strip_prefix(root)` would miss when the OS watcher reports the
///   path through an unresolved symlink alias of the root (notably on
///   macOS, where a `TempDir` root under `/var/folders/...` is the
///   `/private/var/folders/...` form a `canonicalize()` produced and
///   FSEvents reports the unresolved `/var/folders/...` form).
///
/// Returns the resolved absolute path on success.
pub fn resolve_canonical_path(
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

/// Pre-write alias of [`resolve_canonical_path`]. Used by call sites
/// that want the function name to reflect the "we are about to create
/// the file at this path" intent. The algorithm is identical to
/// [`resolve_canonical_path`]; the watcher-side rename / removal
/// dispatch calls the underlying primitive directly because the
/// "we are observing a leaf that has just disappeared" intent is the
/// counterpart to creation and shares the same resolution rules.
pub fn resolve_create_target(
    abs_path: &Path,
    root_canonical: &Path,
) -> Result<PathBuf, CreatePathError> {
    resolve_canonical_path(abs_path, root_canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn to_forward_slash_is_identity_for_unix_relative_paths() {
        assert_eq!(to_forward_slash(Path::new("a/b/c.stmx")), "a/b/c.stmx");
    }

    #[test]
    fn to_forward_slash_handles_simple_filename() {
        assert_eq!(to_forward_slash(Path::new("model.stmx")), "model.stmx");
    }

    #[test]
    fn resolve_existing_inside_root_returns_canonical_path() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let leaf = root.join("model.stmx");
        fs::write(&leaf, b"<root/>").expect("write leaf");

        let resolved = resolve_existing_within_root(&leaf, &root).expect("resolves");
        assert_eq!(resolved, leaf);
    }

    #[test]
    fn resolve_existing_returns_not_found_for_missing_leaf() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let missing = root.join("missing.stmx");

        match resolve_existing_within_root(&missing, &root) {
            Err(ResolutionError::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn resolve_existing_rejects_path_outside_root() {
        // A leaf that exists but lives outside the registry root must not
        // resolve. The traversal from inside the root via `..` must
        // canonicalize to an absolute path that no longer descends from
        // the root.
        let temp = TempDir::new().expect("tempdir");
        let outer = temp.path().canonicalize().expect("canon outer");
        let inner = outer.join("inner");
        fs::create_dir(&inner).expect("mkdir inner");
        let outside = outer.join("escape.stmx");
        fs::write(&outside, b"<root/>").expect("write outside");

        let attempted = inner.join("..").join("escape.stmx");
        match resolve_existing_within_root(&attempted, &inner) {
            Err(ResolutionError::OutOfRoot) => {}
            other => panic!("expected OutOfRoot, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolve_existing_rejects_symlink_pointing_outside_root() {
        // Even a leaf whose path is lexically inside the root must be
        // rejected if symlink resolution lands it outside. Mirrors the
        // create-side test; this is the read-side equivalent.
        let temp = TempDir::new().expect("tempdir");
        let outer = temp.path().canonicalize().expect("canon outer");
        let inner = outer.join("inner");
        fs::create_dir(&inner).expect("mkdir inner");
        let target_outside = outer.join("forbidden.stmx");
        fs::write(&target_outside, b"<root/>").expect("write outside");

        let symlink = inner.join("link.stmx");
        std::os::unix::fs::symlink(&target_outside, &symlink).expect("symlink");

        match resolve_existing_within_root(&symlink, &inner) {
            Err(ResolutionError::OutOfRoot) => {}
            other => panic!("expected OutOfRoot, got {other:?}"),
        }
    }

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
    fn resolves_disappeared_leaf_via_symlinked_root_alias() {
        // Models the macOS FSEvents quirk on Linux. The registry root is
        // canonicalized at server startup, but the watcher's events may
        // arrive at the actor with paths reported through an unresolved
        // symlink alias of the root (FSEvents on macOS does not always
        // resolve `/var/folders/...` to `/private/var/folders/...`). The
        // shared resolution primitive must produce the canonical key for
        // both forms so the registry lookup keys hash identically.
        //
        // Here `canonical_root` is the analogue of macOS
        // `/private/var/folders/...`, and `aliased_root` is a symlink
        // pointing at it (analogous to `/var/folders/...`). A path
        // composed via the alias with a leaf that does not exist on
        // disk must still resolve to the canonical key inside the root.
        let temp = TempDir::new().expect("tempdir");
        let outer = temp.path().canonicalize().expect("canon outer");
        let canonical_root = outer.join("real_root");
        fs::create_dir(&canonical_root).expect("mkdir real root");

        let aliased_root = outer.join("aliased_root");
        std::os::unix::fs::symlink(&canonical_root, &aliased_root).expect("symlink alias");

        // The leaf does not exist on disk — this is the
        // post-removal / rename-source state the watcher observes.
        let leaf_via_alias = aliased_root.join("disappeared.sd.json");
        assert!(!leaf_via_alias.exists(), "precondition: leaf is absent");

        let resolved = resolve_canonical_path(&leaf_via_alias, &canonical_root)
            .expect("resolution succeeds via canonicalize-the-existing-prefix");

        assert!(
            resolved.starts_with(&canonical_root),
            "resolved key must descend from canonical root; got {resolved:?}"
        );
        assert_eq!(
            resolved,
            canonical_root.join("disappeared.sd.json"),
            "resolved key must be the canonical equivalent regardless of which alias the caller passed in"
        );
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
