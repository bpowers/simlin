// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// Disk-write orchestration for the save handler. `resolve_save_target` is
// the pure dispatcher (format -> target shape), `serialize_project` renders
// the bytes, and `commit_write` does the atomic file I/O. Kept together
// because the dispatcher's output is only useful with the writer that
// consumes it.

//! Format-aware write paths for the save handler.
//!
//! Every format is rewritten in place in its own format. XMILE uses
//! `simlin_engine::to_xmile` (byte-stable for round-trips, see
//! `simlin-engine/tests/integration/simulate.rs`); Vensim `.mdl` uses
//! `simlin_engine::to_mdl_with_warnings` (regenerated MDL text including the
//! sketch, with the constructs Vensim cannot express written in their closest
//! form and reported on `WriteOutcome::warnings`); `.sd.json` uses
//! `serde_json::to_string_pretty` for git-friendly line-oriented diffs. All
//! land through the `simlin_engine::io::atomic_write` primitive (sibling
//! tempfile + rename).

use std::path::{Path, PathBuf};

use simlin_engine::datamodel;
use simlin_mcp_core::types::{ErrorOutput, mdl_export_warnings_to_outputs};

use crate::registry::ProjectFormat;

/// Where a save should land on disk and how to format the bytes.
///
/// Every arm overwrites the file the request named: `InPlaceXmile` with
/// regenerated XMILE (`.stmx`/`.xmile`), `InPlaceMdl` with regenerated
/// Vensim text (`.mdl`), and `SdJson` with pretty-printed native JSON
/// (`.sd.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveTarget {
    InPlaceXmile(PathBuf),
    InPlaceMdl(PathBuf),
    SdJson(PathBuf),
}

/// Failure modes for `serialize_project` and `commit_write`. Carries the
/// path that failed so the handler can attribute the cause when it logs.
#[derive(Debug)]
pub enum SaveDiskError {
    XmileSerialize(simlin_engine::Error),
    /// The MDL writer's hard errors: a project Vensim structurally cannot
    /// hold (more than one non-macro model, an ordinary Module variable).
    /// Degraded-but-representable constructs are warnings, not this.
    MdlSerialize(simlin_engine::Error),
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
            SaveDiskError::MdlSerialize(e) => write!(f, "MDL serialize: {e}"),
            SaveDiskError::JsonSerialize(e) => write!(f, "JSON serialize: {e}"),
            SaveDiskError::Io { path, source } => {
                write!(f, "write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for SaveDiskError {}

/// Pure dispatch from `(absolute_path, source_format)` to the
/// `SaveTarget` describing which writer renders the bytes. Every format
/// writes back to `absolute_path` itself.
pub fn resolve_save_target(absolute_path: &Path, source_format: ProjectFormat) -> SaveTarget {
    match source_format {
        ProjectFormat::Stmx | ProjectFormat::Xmile => {
            SaveTarget::InPlaceXmile(absolute_path.to_path_buf())
        }
        ProjectFormat::Mdl => SaveTarget::InPlaceMdl(absolute_path.to_path_buf()),
        ProjectFormat::SdJson => SaveTarget::SdJson(absolute_path.to_path_buf()),
    }
}

/// Outcome of a successful serialization: the path that will land on disk,
/// the exact byte sequence to write, and any non-fatal lossiness warnings
/// the writer raised (only the MDL writer has any today). The caller hashes
/// the bytes for echo-suppression on the file watcher's ingestion path;
/// without the bytes here, the handler would either re-serialize (work
/// duplication, possible drift) or re-read the file (TOCTOU window against
/// the watcher's own event).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    /// Constructs the on-disk format could not express, written in their
    /// closest representable form. Same wire shape as the save handler's
    /// validation errors so the SPA and MCP can render them uniformly.
    pub warnings: Vec<ErrorOutput>,
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
                warnings: Vec::new(),
            })
        }
        SaveTarget::InPlaceMdl(path) => {
            let (text, warnings) = simlin_engine::to_mdl_with_warnings(project)
                .map_err(SaveDiskError::MdlSerialize)?;
            Ok(WriteOutcome {
                path: path.clone(),
                bytes: text.into_bytes(),
                warnings: mdl_export_warnings_to_outputs(project, &warnings),
            })
        }
        SaveTarget::SdJson(path) => {
            let json_str = render_pretty_json(project)?;
            Ok(WriteOutcome {
                path: path.clone(),
                bytes: json_str.into_bytes(),
                warnings: Vec::new(),
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
    fn resolve_target_for_mdl_returns_in_place_mdl() {
        let target = resolve_save_target(Path::new("/tmp/foo/bar.mdl"), ProjectFormat::Mdl);
        assert_eq!(
            target,
            SaveTarget::InPlaceMdl(PathBuf::from("/tmp/foo/bar.mdl"))
        );
    }

    #[test]
    fn resolve_target_for_sd_json_returns_in_place_sd_json() {
        let target = resolve_save_target(Path::new("/tmp/x.sd.json"), ProjectFormat::SdJson);
        assert_eq!(target, SaveTarget::SdJson(PathBuf::from("/tmp/x.sd.json")));
    }

    /// Drive the production two-step (`serialize_project` then
    /// `commit_write`) the save handler runs. The split exists so the
    /// handler can fingerprint the bytes before they land on disk; tests
    /// that only care about the resulting file go through here so they
    /// exercise the same pair of calls production does.
    fn write_through_pipeline(
        project: &datamodel::Project,
        target: &SaveTarget,
    ) -> Result<WriteOutcome, SaveDiskError> {
        let outcome = serialize_project(project, target)?;
        commit_write(&outcome)?;
        Ok(outcome)
    }

    #[test]
    fn save_in_place_xmile_writes_serializable_bytes() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("out.xmile");
        let target = SaveTarget::InPlaceXmile(target_path.clone());
        let project = empty_project();

        let outcome = write_through_pipeline(&project, &target).expect("write succeeds");
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

        write_through_pipeline(&project, &target).expect("write succeeds");

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
    fn save_in_place_xmile_fails_when_parent_dir_missing() {
        let dir = TempDir::new().unwrap();
        let bogus = dir.path().join("nonexistent").join("out.xmile");
        let target = SaveTarget::InPlaceXmile(bogus.clone());
        let project = empty_project();

        let err = write_through_pipeline(&project, &target).unwrap_err();
        match err {
            SaveDiskError::Io { path, .. } => assert_eq!(path, bogus),
            _ => panic!("expected SaveDiskError::Io, got {err:?}"),
        }
    }

    fn load_teacup_mdl_project() -> datamodel::Project {
        let mdl_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("teacup.mdl");
        let contents = fs::read_to_string(&mdl_path).unwrap_or_else(|e| {
            panic!("read fixture {}: {e}", mdl_path.display());
        });
        simlin_engine::open_vensim(&contents).expect("teacup.mdl parses")
    }

    /// An `.mdl` save overwrites the `.mdl` itself with regenerated Vensim
    /// text (sketch included) that the MDL reader parses back to the same
    /// variables and view elements; no sibling file appears.
    #[test]
    fn save_in_place_mdl_rewrites_vensim_text_that_round_trips() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("teacup.mdl");
        fs::write(&target_path, b"{UTF-8}\n\nplaceholder=1\n  ~\n  ~|\n").unwrap();
        let target = SaveTarget::InPlaceMdl(target_path.clone());
        let project = load_teacup_mdl_project();

        let outcome = write_through_pipeline(&project, &target).expect("write succeeds");
        assert_eq!(
            outcome.path, target_path,
            "writer must target the .mdl itself"
        );
        assert!(
            outcome.warnings.is_empty(),
            "teacup.mdl is fully Vensim-expressible: {:?}",
            outcome.warnings
        );
        assert!(
            !dir.path().join("teacup.sd.json").exists(),
            "no sidecar may be created"
        );

        let bytes = fs::read(&target_path).unwrap();
        assert_eq!(outcome.bytes, bytes);
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.starts_with("{UTF-8}"), "file must be MDL text");
        assert!(
            text.contains("Sketch information"),
            "sketch must be written"
        );

        let reparsed = simlin_engine::open_vensim(text).expect("reparses as Vensim");
        let names = |p: &datamodel::Project| -> Vec<String> {
            let mut v: Vec<String> = p.models[0]
                .variables
                .iter()
                .map(|v| v.get_ident().to_string())
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(&reparsed), names(&project));
        let element_count = |p: &datamodel::Project| -> usize {
            p.models[0]
                .views
                .iter()
                .map(|v| match v {
                    datamodel::View::StockFlow(sf) => sf.elements.len(),
                })
                .sum()
        };
        assert!(element_count(&project) > 0, "fixture must carry a sketch");
        assert_eq!(element_count(&reparsed), element_count(&project));
    }

    /// The MDL writer's lossiness channel reaches the outcome: a project
    /// carrying a construct Vensim cannot express (a non-negative flow,
    /// which the SPA can set through `compat.nonNegative`) still writes,
    /// and the degradation is reported as an `MDL export:` warning rather
    /// than a failure.
    #[test]
    fn save_in_place_mdl_reports_lossiness_as_warnings_and_still_writes() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("teacup.mdl");
        let target = SaveTarget::InPlaceMdl(target_path.clone());
        let mut project = load_teacup_mdl_project();
        let flow = project.models[0]
            .variables
            .iter_mut()
            .find_map(|v| match v {
                datamodel::Variable::Flow(f) => Some(f),
                _ => None,
            })
            .expect("teacup has a flow");
        flow.compat.non_negative = true;

        let outcome = write_through_pipeline(&project, &target).expect("lossy write succeeds");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|w| w.message.starts_with("MDL export:")),
            "the dropped non-negative flag must be reported: {:?}",
            outcome.warnings
        );
        assert!(target_path.is_file(), "the write must still land");
        let text = fs::read_to_string(&target_path).unwrap();
        simlin_engine::open_vensim(&text).expect("degraded output still parses");
    }

    /// The writer's hard errors fail the save: Vensim has no representation
    /// for a multi-model project, so writing one to `.mdl` is an
    /// `MdlSerialize` error and nothing lands on disk.
    #[test]
    fn save_in_place_mdl_fails_for_a_multi_model_project_without_writing() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("two.mdl");
        let target = SaveTarget::InPlaceMdl(target_path.clone());
        let mut project = empty_project();
        let mut second = project.models[0].clone();
        second.name = "second".to_string();
        project.models.push(second);

        let err = write_through_pipeline(&project, &target).unwrap_err();
        assert!(
            matches!(err, SaveDiskError::MdlSerialize(_)),
            "expected MdlSerialize, got {err:?}"
        );
        assert!(!target_path.exists(), "a failed serialize must not write");
    }

    #[test]
    fn save_sd_json_writes_in_place() {
        let dir = TempDir::new().unwrap();
        let target_path = dir.path().join("model.sd.json");
        let target = SaveTarget::SdJson(target_path.clone());
        let project = empty_project();

        let outcome = write_through_pipeline(&project, &target).expect("write succeeds");
        assert_eq!(outcome.path, target_path);

        let bytes = fs::read(&target_path).unwrap();
        assert_eq!(outcome.bytes, bytes);
        let json_project: simlin_engine::json::Project =
            serde_json::from_slice(&bytes).expect("sd.json parses back");
        let reparsed: datamodel::Project = json_project.into();
        assert_eq!(reparsed.name, project.name);
    }
}
