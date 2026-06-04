// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// Disk-write orchestration for the save handler. `resolve_save_target` is
// the pure dispatcher (format -> target shape); `save_to_disk` does the
// actual atomic file I/O. Kept together because the dispatcher's output is
// only useful with the writer that consumes it.

//! Format-aware write paths for the save handler.
//!
//! XMILE in-place writes use `simlin_engine::to_xmile` (byte-stable for
//! round-trips, see `simlin-engine/tests/integration/simulate.rs`) plus the
//! `simlin_engine::io::atomic_write` primitive (sibling tempfile + rename).
//! `.sd.json` writes use `serde_json::to_string_pretty` for git-friendly
//! line-oriented diffs.

use std::path::{Path, PathBuf};

use simlin_engine::datamodel;

use crate::registry::ProjectFormat;

/// Where a save should land on disk and how to format the bytes.
///
/// `InPlaceXmile` overwrites the original `.stmx`/`.xmile` file with
/// regenerated XMILE. `SidecarJson` is the `.mdl` path: we never modify
/// the `.mdl`; the new state lands in a sibling `.sd.json` that becomes
/// source-of-truth on subsequent reads (the GET handler already prefers
/// the sidecar when both exist). `SdJson` overwrites an existing
/// `.sd.json` directly (no `.mdl` involved).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveTarget {
    InPlaceXmile(PathBuf),
    SidecarJson {
        mdl_path: PathBuf,
        sidecar_path: PathBuf,
    },
    SdJson(PathBuf),
}

/// Failure modes for `save_to_disk`. Carries the path that failed so the
/// handler can attribute the cause when it logs.
#[derive(Debug)]
pub enum SaveDiskError {
    XmileSerialize(simlin_engine::Error),
    JsonSerialize(serde_json::Error),
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for SaveDiskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveDiskError::XmileSerialize(e) => write!(f, "XMILE serialize: {e:?}"),
            SaveDiskError::JsonSerialize(e) => write!(f, "JSON serialize: {e}"),
            SaveDiskError::Io { path, source } => {
                write!(f, "write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for SaveDiskError {}

/// Pure dispatch from `(absolute_path, source_format)` to the
/// `SaveTarget` describing where the bytes go and in which format.
///
/// Sole owner of the sidecar-naming convention for `.mdl`; keep the rule
/// here so `handlers.rs` doesn't grow a duplicate copy.
pub fn resolve_save_target(absolute_path: &Path, source_format: ProjectFormat) -> SaveTarget {
    match source_format {
        ProjectFormat::Stmx | ProjectFormat::Xmile => {
            SaveTarget::InPlaceXmile(absolute_path.to_path_buf())
        }
        ProjectFormat::Mdl => {
            let sidecar_path = sidecar_for_mdl(absolute_path);
            SaveTarget::SidecarJson {
                mdl_path: absolute_path.to_path_buf(),
                sidecar_path,
            }
        }
        ProjectFormat::SdJson => SaveTarget::SdJson(absolute_path.to_path_buf()),
    }
}

/// Outcome of a successful disk write: the path that landed on disk plus
/// the exact byte sequence that was written. The caller hashes those
/// bytes for echo-suppression on the file watcher's ingestion path
/// (Phase 4); without the bytes here, the handler would either re-serialize
/// (work duplication, possible drift) or re-read the file (TOCTOU window
/// against the watcher's own event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

/// Serialize `project` to the byte representation implied by `target`
/// without touching the filesystem. The returned `WriteOutcome` carries
/// the target path and the serialized bytes.
///
/// Callers that need the hash for echo-suppression before the disk write
/// (to close the watcher-fires-before-hash-is-stored race) should call
/// this, record the hash, then call `commit_write` to flush the bytes.
pub fn serialize_project(
    project: &datamodel::Project,
    target: &SaveTarget,
) -> Result<WriteOutcome, SaveDiskError> {
    match target {
        SaveTarget::InPlaceXmile(path) => {
            let xmile = simlin_engine::to_xmile(project).map_err(SaveDiskError::XmileSerialize)?;
            Ok(WriteOutcome {
                path: path.clone(),
                bytes: xmile.into_bytes(),
            })
        }
        SaveTarget::SidecarJson { sidecar_path, .. } => {
            let json_str = render_pretty_json(project)?;
            Ok(WriteOutcome {
                path: sidecar_path.clone(),
                bytes: json_str.into_bytes(),
            })
        }
        SaveTarget::SdJson(path) => {
            let json_str = render_pretty_json(project)?;
            Ok(WriteOutcome {
                path: path.clone(),
                bytes: json_str.into_bytes(),
            })
        }
    }
}

/// Flush a `WriteOutcome`'s bytes to disk atomically (tempfile + rename).
/// Counterpart to `serialize_project`; together they give callers the
/// ability to precompute the hash and update registry state before the
/// OS-visible write event fires.
pub fn commit_write(outcome: &WriteOutcome) -> Result<(), SaveDiskError> {
    atomic_write_to(&outcome.path, &outcome.bytes)
}

/// Serialize `project` into the format implied by `target`, then write
/// it atomically. Returns the path that was written and the exact bytes
/// emitted, so the caller can both stat for registry metadata and
/// fingerprint for echo suppression in a single pass.
///
/// `SidecarJson` writes only the sidecar; the `.mdl` is never modified
/// (the design's "sidecar becomes the new source of truth once it
/// exists" rule, codified at the writer layer).
pub fn save_to_disk(
    project: &datamodel::Project,
    target: &SaveTarget,
) -> Result<WriteOutcome, SaveDiskError> {
    match target {
        SaveTarget::InPlaceXmile(path) => {
            let xmile = simlin_engine::to_xmile(project).map_err(SaveDiskError::XmileSerialize)?;
            let bytes = xmile.into_bytes();
            atomic_write_to(path, &bytes)?;
            Ok(WriteOutcome {
                path: path.clone(),
                bytes,
            })
        }
        SaveTarget::SidecarJson { sidecar_path, .. } => {
            let json_str = render_pretty_json(project)?;
            let bytes = json_str.into_bytes();
            atomic_write_to(sidecar_path, &bytes)?;
            Ok(WriteOutcome {
                path: sidecar_path.clone(),
                bytes,
            })
        }
        SaveTarget::SdJson(path) => {
            let json_str = render_pretty_json(project)?;
            let bytes = json_str.into_bytes();
            atomic_write_to(path, &bytes)?;
            Ok(WriteOutcome {
                path: path.clone(),
                bytes,
            })
        }
    }
}

fn atomic_write_to(path: &Path, bytes: &[u8]) -> Result<(), SaveDiskError> {
    simlin_engine::io::atomic_write(path, bytes).map_err(|source| SaveDiskError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Pretty-printed JSON is chosen for git-friendliness (line-oriented
/// diffs); we can switch to compact later if file size becomes an issue.
fn render_pretty_json(project: &datamodel::Project) -> Result<String, SaveDiskError> {
    let json_project = simlin_engine::json::Project::from(project);
    serde_json::to_string_pretty(&json_project).map_err(SaveDiskError::JsonSerialize)
}

/// For `path = "/dir/foo.mdl"`, return `/dir/foo.sd.json`. Mirrors the
/// rule used by the GET handler when picking up an existing sidecar; the
/// two must stay in lock-step or a save would land in a path the next
/// GET wouldn't read from.
fn sidecar_for_mdl(mdl_path: &Path) -> PathBuf {
    let parent = mdl_path.parent().unwrap_or_else(|| Path::new(""));
    let stem = mdl_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    parent.join(format!("{stem}.sd.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn load_teacup_project() -> datamodel::Project {
        let xmile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("teacup.xmile");
        let contents = fs::read_to_string(&xmile_path).unwrap_or_else(|e| {
            panic!("read fixture {}: {e}", xmile_path.display());
        });
        let mut reader = Cursor::new(contents.as_bytes());
        simlin_engine::open_xmile(&mut reader).expect("teacup.xmile parses")
    }

    fn empty_project() -> datamodel::Project {
        let json_body = r#"{
            "name": "tiny",
            "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
            "models": [{"name": "main"}]
        }"#;
        let json_project: simlin_engine::json::Project =
            serde_json::from_str(json_body).expect("test fixture parses");
        json_project.into()
    }

    #[test]
    fn resolve_target_for_xmile_returns_in_place() {
        let target = resolve_save_target(Path::new("/tmp/x.xmile"), ProjectFormat::Xmile);
        assert_eq!(
            target,
            SaveTarget::InPlaceXmile(PathBuf::from("/tmp/x.xmile"))
        );
    }

    #[test]
    fn resolve_target_for_stmx_returns_in_place() {
        let target = resolve_save_target(Path::new("/tmp/x.stmx"), ProjectFormat::Stmx);
        assert_eq!(
            target,
            SaveTarget::InPlaceXmile(PathBuf::from("/tmp/x.stmx"))
        );
    }

    #[test]
    fn resolve_target_for_mdl_returns_sidecar_pair() {
        let target = resolve_save_target(Path::new("/tmp/foo/bar.mdl"), ProjectFormat::Mdl);
        assert_eq!(
            target,
            SaveTarget::SidecarJson {
                mdl_path: PathBuf::from("/tmp/foo/bar.mdl"),
                sidecar_path: PathBuf::from("/tmp/foo/bar.sd.json"),
            }
        );
    }

    #[test]
    fn resolve_target_for_sd_json_returns_in_place_sd_json() {
        let target = resolve_save_target(Path::new("/tmp/x.sd.json"), ProjectFormat::SdJson);
        assert_eq!(target, SaveTarget::SdJson(PathBuf::from("/tmp/x.sd.json")));
    }

    #[test]
    fn save_in_place_xmile_writes_serializable_bytes() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("out.xmile");
        let target = SaveTarget::InPlaceXmile(target_path.clone());
        let project = empty_project();

        let outcome = save_to_disk(&project, &target).expect("write succeeds");
        assert_eq!(outcome.path, target_path);

        let bytes = fs::read(&target_path).expect("file exists");
        // Outcome carries the same bytes that landed on disk so the caller
        // doesn't need a separate fs::read() to fingerprint them.
        assert_eq!(outcome.bytes, bytes);
        let mut reader = Cursor::new(&bytes[..]);
        let reparsed = simlin_engine::open_xmile(&mut reader).expect("reparses");
        assert_eq!(reparsed.name, project.name);
        assert_eq!(reparsed.models.len(), project.models.len());
    }

    #[test]
    fn save_in_place_xmile_round_trip_preserves_structure_for_real_model() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("teacup.xmile");
        let target = SaveTarget::InPlaceXmile(target_path.clone());
        let project = load_teacup_project();

        save_to_disk(&project, &target).expect("write succeeds");

        let bytes = fs::read(&target_path).unwrap();
        let mut reader = Cursor::new(&bytes[..]);
        let reparsed = simlin_engine::open_xmile(&mut reader).expect("reparses");

        let original_json = simlin_engine::json::Project::from(&project);
        let reparsed_json = simlin_engine::json::Project::from(&reparsed);
        let original_str = serde_json::to_string(&original_json).unwrap();
        let reparsed_str = serde_json::to_string(&reparsed_json).unwrap();
        assert_eq!(original_str, reparsed_str);
    }

    #[test]
    fn save_in_place_xmile_is_byte_stable_across_writes() {
        let dir = TempDir::new().unwrap();
        let path_a = dir.path().join("a.xmile");
        let path_b = dir.path().join("b.xmile");
        let project = load_teacup_project();

        save_to_disk(&project, &SaveTarget::InPlaceXmile(path_a.clone())).unwrap();
        save_to_disk(&project, &SaveTarget::InPlaceXmile(path_b.clone())).unwrap();

        let bytes_a = fs::read(&path_a).unwrap();
        let bytes_b = fs::read(&path_b).unwrap();
        assert_eq!(
            bytes_a, bytes_b,
            "XMILE serialization must be byte-stable for the same input"
        );
    }

    #[test]
    fn save_in_place_xmile_fails_when_parent_dir_missing() {
        let dir = TempDir::new().unwrap();
        let bogus = dir.path().join("nonexistent").join("out.xmile");
        let target = SaveTarget::InPlaceXmile(bogus.clone());
        let project = empty_project();

        let err = save_to_disk(&project, &target).unwrap_err();
        match err {
            SaveDiskError::Io { path, .. } => assert_eq!(path, bogus),
            _ => panic!("expected SaveDiskError::Io, got {err:?}"),
        }
    }

    #[test]
    fn save_sidecar_json_writes_to_sidecar_and_leaves_mdl_alone() {
        let dir = TempDir::new().unwrap();
        let mdl_path = dir.path().join("model.mdl");
        let sidecar_path = dir.path().join("model.sd.json");

        // Write a stub .mdl content; the writer must not touch it.
        let original_mdl_bytes = b"{UTF-8}\n\nplaceholder=1\n  ~\n  ~|\n";
        fs::write(&mdl_path, original_mdl_bytes).unwrap();

        let target = SaveTarget::SidecarJson {
            mdl_path: mdl_path.clone(),
            sidecar_path: sidecar_path.clone(),
        };
        let project = empty_project();
        let outcome = save_to_disk(&project, &target).expect("write succeeds");
        assert_eq!(
            outcome.path, sidecar_path,
            "writer must return the sidecar path"
        );

        // The .mdl file must be byte-identical to what we wrote.
        let post_mdl = fs::read(&mdl_path).unwrap();
        assert_eq!(
            post_mdl,
            original_mdl_bytes.as_ref(),
            ".mdl file must not be modified by a sidecar write"
        );

        // The sidecar must contain valid JSON that round-trips back to the
        // input project.
        let sidecar_bytes = fs::read(&sidecar_path).unwrap();
        let json_project: simlin_engine::json::Project =
            serde_json::from_slice(&sidecar_bytes).expect("sidecar parses");
        let reparsed: datamodel::Project = json_project.into();
        assert_eq!(reparsed.name, project.name);
        assert_eq!(reparsed.models.len(), project.models.len());
    }

    #[test]
    fn save_sidecar_json_writes_pretty_printed_content() {
        // Pretty-print is the design choice for git-friendliness; if it
        // ever silently switches to compact, this test catches the drift.
        let dir = TempDir::new().unwrap();
        let mdl_path = dir.path().join("model.mdl");
        let sidecar_path = dir.path().join("model.sd.json");
        fs::write(&mdl_path, b"placeholder").unwrap();

        let target = SaveTarget::SidecarJson {
            mdl_path,
            sidecar_path: sidecar_path.clone(),
        };
        let project = empty_project();
        save_to_disk(&project, &target).unwrap();

        let bytes = fs::read(&sidecar_path).unwrap();
        let s = std::str::from_utf8(&bytes).unwrap();
        // Pretty JSON contains newlines + indentation; compact would not.
        assert!(s.contains('\n'), "sidecar must be pretty-printed");
    }

    #[test]
    fn save_sd_json_writes_in_place() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("model.sd.json");
        let target = SaveTarget::SdJson(target_path.clone());
        let project = empty_project();

        let outcome = save_to_disk(&project, &target).expect("write succeeds");
        assert_eq!(outcome.path, target_path);

        let bytes = fs::read(&target_path).unwrap();
        assert_eq!(outcome.bytes, bytes);
        let json_project: simlin_engine::json::Project =
            serde_json::from_slice(&bytes).expect("sd.json parses back");
        let reparsed: datamodel::Project = json_project.into();
        assert_eq!(reparsed.name, project.name);
    }

    #[test]
    fn save_sd_json_overwrites_existing_file_idempotently() {
        // Saving twice must produce identical bytes (writer is byte-stable
        // for the same input regardless of prior state).
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("model.sd.json");
        // Pre-seed with arbitrary stale bytes to confirm overwrite works.
        fs::write(&target_path, b"stale").unwrap();
        let project = empty_project();

        save_to_disk(&project, &SaveTarget::SdJson(target_path.clone())).unwrap();
        let bytes_first = fs::read(&target_path).unwrap();
        save_to_disk(&project, &SaveTarget::SdJson(target_path.clone())).unwrap();
        let bytes_second = fs::read(&target_path).unwrap();
        assert_eq!(
            bytes_first, bytes_second,
            "JSON serialization must be byte-stable for the same input"
        );
    }
}
