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
//!    Implemented by [`resolve_create_target`].
//! 3. **Resolving the registry's canonical key** — for `.mdl` paths, when a
//!    sibling `.sd.json` exists on disk it becomes the canonical entry for
//!    both reads and writes. The sidecar's canonical form must itself
//!    descend from the registry root: a symlink sidecar pointing outside
//!    the tree falls back to the `.mdl`. Implemented by
//!    [`apply_sidecar_preference`].
//!
//! Centralizing these here removes the class of bug "consumer X forgot to
//! apply the rule consumer Y enforces": the rules are implemented once and
//! every consumer calls the same function. Trivial helpers
//! ([`sidecar_for_mdl`], [`is_mdl_extension`], [`to_forward_slash`]) live
//! here for the same reason — different consumers had drifted on case
//! folding and string-rendering, producing the same shape of bug.

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

/// Sibling `.sd.json` for a `.mdl` path: `/dir/foo.mdl` → `/dir/foo.sd.json`.
///
/// Lossy conversion of the file stem to UTF-8 is intentional: filenames that
/// are not valid UTF-8 produce an empty stem, so the resulting sidecar path
/// is `<parent>/.sd.json`. That is fine because such paths cannot be created
/// by callers in practice (the registry rejects them upstream) and the
/// callers that do reach here use the result only to test on-disk existence.
pub fn sidecar_for_mdl(mdl_path: &Path) -> PathBuf {
    let parent = mdl_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = mdl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    parent.join(format!("{stem}.sd.json"))
}

/// Case-insensitive check for the `.mdl` extension.
///
/// Used by the consumers that distinguish "is this an `.mdl`?" from "what
/// is the on-disk format here?" — for the latter the heavier
/// [`crate::registry::ProjectFormat`]-yielding dispatcher is correct, but
/// the sidecar-preference rule only cares whether the input is an `.mdl`
/// at all. Centralising here ensures every consumer agrees on case
/// folding.
pub fn is_mdl_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("mdl"))
        .unwrap_or(false)
}

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

/// Result of [`apply_sidecar_preference`]. `path` is what the consumer
/// should use as the registry key going forward; `redirected_to_sidecar`
/// is true when the input was a `.mdl` whose sibling `.sd.json` was
/// resolved to in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKey {
    /// The canonical path consumers should use as the registry key. Either
    /// the input (when no redirect happened) or the canonicalized sidecar.
    pub path: PathBuf,
    /// True iff the input was a `.mdl` whose sibling `.sd.json` exists on
    /// disk (and resolved cleanly inside the root); false in every other
    /// case.
    pub redirected_to_sidecar: bool,
}

/// Apply the `.mdl` sidecar-preference rule to `canonical_input`,
/// returning the canonical registry key the consumer should use.
///
/// The contract:
///
/// - Non-`.mdl` inputs return as-is, with `redirected_to_sidecar =
///   false`.
/// - `.mdl` inputs without a sibling `.sd.json` return as-is.
/// - `.mdl` inputs whose sibling `.sd.json` exists *and* canonicalizes
///   to a path inside `root_canonical` return the sidecar's canonical
///   path with `redirected_to_sidecar = true`.
/// - `.mdl` inputs whose sibling sidecar exists but cannot be
///   canonicalized (TOCTOU vanish, EACCES) or whose canonical form is
///   outside the registry root fall back to the input. Returning the
///   `.mdl` is the safe choice: the in-place .mdl is already known to
///   live under the root (see [`resolve_existing_within_root`]'s
///   contract), and silently using an out-of-root sidecar would let a
///   malicious or misconfigured symlink redirect reads and writes
///   outside the watched tree.
///
/// This is the single point where the sidecar-preference rule is
/// implemented; every consumer (HTTP `get_project` / `save_project`,
/// MCP `open` / `save`, and the watcher's `.mdl`-with-sidecar skip) is
/// expected to call through here so a future contributor cannot
/// re-introduce the divergent rule that produced ~5 P1 review bugs in
/// PR #476.
///
/// `canonical_input` MUST already be a canonicalized absolute path
/// inside `root_canonical`. The caller is expected to have run
/// [`resolve_existing_within_root`] (or equivalent) first; the sidecar
/// step is an *additional* rule on top of the boundary check.
pub fn apply_sidecar_preference(canonical_input: &Path, root_canonical: &Path) -> ResolvedKey {
    if !is_mdl_extension(canonical_input) {
        return ResolvedKey {
            path: canonical_input.to_path_buf(),
            redirected_to_sidecar: false,
        };
    }
    let sidecar = sidecar_for_mdl(canonical_input);
    if !sidecar.is_file() {
        return ResolvedKey {
            path: canonical_input.to_path_buf(),
            redirected_to_sidecar: false,
        };
    }
    match sidecar.canonicalize() {
        Ok(canonical_sidecar) if canonical_sidecar.starts_with(root_canonical) => ResolvedKey {
            path: canonical_sidecar,
            redirected_to_sidecar: true,
        },
        // Any failure mode (TOCTOU vanish, EACCES, sidecar canonicalizing
        // outside the root) falls back to the .mdl. The latter case is
        // the "malicious symlink sidecar" defence — without this fall-back
        // a save handler would happily write the user's content to the
        // canonical sidecar destination outside the watched tree.
        Ok(_) | Err(_) => ResolvedKey {
            path: canonical_input.to_path_buf(),
            redirected_to_sidecar: false,
        },
    }
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
    fn sidecar_for_mdl_swaps_extension_to_sd_json() {
        assert_eq!(
            sidecar_for_mdl(Path::new("/tmp/foo/bar.mdl")),
            PathBuf::from("/tmp/foo/bar.sd.json")
        );
    }

    #[test]
    fn sidecar_for_mdl_preserves_parent_directory_chain() {
        // Multi-level paths must keep their parent intact; only the leaf
        // file changes.
        assert_eq!(
            sidecar_for_mdl(Path::new("a/b/c/model.mdl")),
            PathBuf::from("a/b/c/model.sd.json")
        );
    }

    #[test]
    fn sidecar_for_mdl_handles_dotted_stem() {
        // `foo.bar.mdl` -> stem `foo.bar`, so the sidecar is
        // `foo.bar.sd.json`. This matches the rest of the pipeline's
        // file_stem-based logic.
        assert_eq!(
            sidecar_for_mdl(Path::new("/tmp/foo.bar.mdl")),
            PathBuf::from("/tmp/foo.bar.sd.json")
        );
    }

    #[test]
    fn is_mdl_extension_matches_lowercase() {
        assert!(is_mdl_extension(Path::new("/tmp/x.mdl")));
    }

    #[test]
    fn is_mdl_extension_matches_uppercase_and_mixed() {
        // The watcher and HTTP handlers see paths produced by
        // canonicalize() (which preserves case on case-sensitive
        // filesystems and lower-cases on case-insensitive ones), so an
        // uppercase `.MDL` from a case-preserving fs must classify
        // identically. This is the exact bug shape the centralisation
        // is designed to prevent.
        assert!(is_mdl_extension(Path::new("/tmp/X.MDL")));
        assert!(is_mdl_extension(Path::new("/tmp/x.Mdl")));
    }

    #[test]
    fn is_mdl_extension_rejects_other_extensions() {
        assert!(!is_mdl_extension(Path::new("/tmp/x.stmx")));
        assert!(!is_mdl_extension(Path::new("/tmp/x.sd.json")));
        assert!(!is_mdl_extension(Path::new("/tmp/x.xmile")));
        assert!(!is_mdl_extension(Path::new("/tmp/x.txt")));
    }

    #[test]
    fn is_mdl_extension_rejects_no_extension() {
        assert!(!is_mdl_extension(Path::new("/tmp/no_extension")));
    }

    #[test]
    fn to_forward_slash_is_identity_for_unix_relative_paths() {
        assert_eq!(to_forward_slash(Path::new("a/b/c.stmx")), "a/b/c.stmx");
    }

    #[test]
    fn to_forward_slash_handles_simple_filename() {
        assert_eq!(to_forward_slash(Path::new("model.stmx")), "model.stmx");
    }

    #[test]
    fn sidecar_preference_returns_input_for_non_mdl() {
        // .stmx, .xmile, .sd.json, .xml, etc. — none of them apply the
        // sidecar redirect; they return the input unchanged. Pure
        // function: no filesystem involvement here.
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let input = root.join("model.stmx");

        let resolved = apply_sidecar_preference(&input, &root);
        assert_eq!(resolved.path, input);
        assert!(!resolved.redirected_to_sidecar);
    }

    #[test]
    fn sidecar_preference_returns_input_when_mdl_has_no_sidecar() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let mdl = root.join("model.mdl");
        fs::write(&mdl, b"{UTF-8}\n\n").expect("write mdl");
        // No sidecar file; preference should return the .mdl unchanged.

        let resolved = apply_sidecar_preference(&mdl, &root);
        assert_eq!(resolved.path, mdl);
        assert!(!resolved.redirected_to_sidecar);
    }

    #[test]
    fn sidecar_preference_routes_to_sidecar_when_present() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let mdl = root.join("model.mdl");
        let sidecar = root.join("model.sd.json");
        fs::write(&mdl, b"{UTF-8}\n\n").expect("write mdl");
        fs::write(&sidecar, b"{}").expect("write sidecar");

        let resolved = apply_sidecar_preference(&mdl, &root);
        assert_eq!(
            resolved.path, sidecar,
            "sidecar canonical path must be returned"
        );
        assert!(resolved.redirected_to_sidecar);
    }

    #[test]
    fn sidecar_preference_handles_uppercase_mdl_extension() {
        // Case-insensitive .mdl matching is the rule the centralization
        // pins; an uppercase .MDL with a sidecar must redirect just like
        // a lowercase .mdl. Without this every consumer's case-folding
        // policy would have to agree independently — the bug shape we
        // are eliminating.
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().canonicalize().expect("canon root");
        let mdl = root.join("model.MDL");
        let sidecar = root.join("model.sd.json");
        fs::write(&mdl, b"{UTF-8}\n\n").expect("write mdl");
        fs::write(&sidecar, b"{}").expect("write sidecar");

        let resolved = apply_sidecar_preference(&mdl, &root);
        assert_eq!(resolved.path, sidecar);
        assert!(resolved.redirected_to_sidecar);
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_preference_falls_back_when_sidecar_symlinks_outside_root() {
        // A sidecar that is a symlink whose target lives outside the
        // registry root must NOT redirect — neither HTTP nor MCP wants
        // to read or write content outside the watched tree. Falling
        // back to the .mdl preserves a usable state without leaking
        // out-of-tree content.
        let temp = TempDir::new().expect("tempdir");
        let outer = temp.path().canonicalize().expect("canon outer");
        let inner = outer.join("inner");
        fs::create_dir(&inner).expect("mkdir inner");

        let mdl = inner.join("model.mdl");
        fs::write(&mdl, b"{UTF-8}\n\n").expect("write mdl");

        let outside_target = outer.join("escaped.sd.json");
        fs::write(&outside_target, b"{}").expect("write outside target");
        let sidecar = inner.join("model.sd.json");
        std::os::unix::fs::symlink(&outside_target, &sidecar).expect("symlink");

        let resolved = apply_sidecar_preference(&mdl, &inner);
        assert_eq!(
            resolved.path, mdl,
            "out-of-root symlink sidecar must fall back to the .mdl"
        );
        assert!(
            !resolved.redirected_to_sidecar,
            "redirect flag must be false when fallback fires"
        );
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
