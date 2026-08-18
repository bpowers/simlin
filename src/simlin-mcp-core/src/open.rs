// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
//! Project parsing helpers shared between MCP tools.
//!
//! These helpers operate on bytes already loaded from a backing store:
//! callers (such as a `ProjectAccess` impl) own the I/O, and pass the
//! contents in.  This keeps the JSON/XMILE/Vensim parsing logic — and
//! its substantial test surface — independent of the filesystem so the
//! same code paths run unchanged when the registry-backed impl in
//! Phase 6 hands over an in-memory string.

use std::io::BufReader;
use std::path::Path;

use anyhow::Context as _;

use crate::errors::AccessError;
use crate::types::SourceFormat;

/// Resolve the model name to use, falling back to the first model when the
/// requested name is "main" and no model is literally named "main".
///
/// This allows tools to use "main" as a default that works for both
/// projects with a model named "main" and single-model projects imported
/// from XMILE or Vensim where the model may have a different name.
pub fn resolve_model_name<'a>(
    project: &'a simlin_engine::datamodel::Project,
    requested: &'a str,
) -> &'a str {
    if let Some(m) = project.get_model(requested) {
        // get_model handles the empty-name/"main" alias; return the actual
        // stored name so downstream callers (patch application) can do an
        // exact match.
        return &m.name;
    }
    if requested == "main"
        && let Some(first) = project.models.first()
    {
        return &first.name;
    }
    requested
}

/// Ensure every variable in every model of the project has a UID.
///
/// Variables parsed from some file formats (SD-AI, older JSON files without
/// UIDs) may arrive with `uid: None`.  Any operation that needs to reference
/// variables by UID (e.g. `SetLoopName`) will fail on those variables.  We
/// assign UIDs eagerly at open time so callers never need to guard against
/// the missing-UID case.
///
/// We compute a single high-water-mark across both variable UIDs and view
/// element UIDs for each model to guarantee uniqueness.
fn ensure_variable_uids(project: &mut simlin_engine::datamodel::Project) {
    for model in &mut project.models {
        let max_var_uid = model
            .variables
            .iter()
            .filter_map(|v| match v {
                simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                simlin_engine::datamodel::Variable::Module(m) => m.uid,
            })
            .max()
            .unwrap_or(0);
        let max_view_uid = model
            .views
            .iter()
            .flat_map(|v| match v {
                simlin_engine::datamodel::View::StockFlow(sf) => sf.elements.iter(),
            })
            .map(|e| e.get_uid())
            .max()
            .unwrap_or(0);
        let mut next_uid = max_var_uid.max(max_view_uid) + 1;

        for var in &mut model.variables {
            let has_uid = match var {
                simlin_engine::datamodel::Variable::Stock(s) => s.uid.is_some(),
                simlin_engine::datamodel::Variable::Flow(f) => f.uid.is_some(),
                simlin_engine::datamodel::Variable::Aux(a) => a.uid.is_some(),
                simlin_engine::datamodel::Variable::Module(m) => m.uid.is_some(),
            };
            if !has_uid {
                match var {
                    simlin_engine::datamodel::Variable::Stock(s) => s.uid = Some(next_uid),
                    simlin_engine::datamodel::Variable::Flow(f) => f.uid = Some(next_uid),
                    simlin_engine::datamodel::Variable::Aux(a) => a.uid = Some(next_uid),
                    simlin_engine::datamodel::Variable::Module(m) => m.uid = Some(next_uid),
                }
                next_uid += 1;
            }
        }
    }
}

/// The [`SourceFormat`] a path's extension commits to, or `None` for the
/// extensions (`.json`, `.sd.json`, anything else) whose format is decided
/// by content.
///
/// This is the single extension dispatcher for the MCP surface: `open_project`
/// uses it to pick a parser and `create_model` uses it to pick the writer for
/// a fresh file, so a `.mdl` created through `CreateModel` is written as
/// Vensim text that `ReadModel` can parse back rather than JSON under a
/// misleading extension.
pub fn format_for_extension(path: &Path) -> Option<SourceFormat> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "stmx" | "xmile" | "xml" => Some(SourceFormat::Xmile),
        "mdl" => Some(SourceFormat::Mdl),
        _ => None,
    }
}

/// Open a project from file contents.  XMILE and Vensim formats are
/// detected by extension; JSON files use content-based detection
/// (top-level `models` key = native, `variables` key = SD-AI).
///
/// Parse failures are surfaced as `AccessError::ParseError` so callers
/// can pattern-match on the trait error without having to inspect a
/// generic `anyhow::Error`.  I/O is the caller's responsibility (see
/// `ProjectAccess::open`); this function takes the bytes directly.
pub fn open_project(
    path: &Path,
    contents: &str,
) -> Result<(simlin_engine::datamodel::Project, SourceFormat), AccessError> {
    let (mut project, format) = match format_for_extension(path) {
        Some(SourceFormat::Xmile) => {
            let mut reader = BufReader::new(contents.as_bytes());
            let project = simlin_engine::open_xmile(&mut reader).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to parse XMILE: {e:?}"))
            })?;
            (project, SourceFormat::Xmile)
        }
        Some(SourceFormat::Mdl) => {
            let project = simlin_engine::open_vensim(contents).map_err(|e| {
                AccessError::ParseError(anyhow::anyhow!("failed to parse Vensim: {e:?}"))
            })?;
            (project, SourceFormat::Mdl)
        }
        Some(SourceFormat::NativeJson | SourceFormat::SdaiJson) => {
            unreachable!("format_for_extension never commits to a JSON format")
        }
        None => {
            let v: serde_json::Value = serde_json::from_str(contents)
                .context("failed to parse JSON")
                .map_err(AccessError::ParseError)?;
            if v.get("models").is_some() {
                let json_project: simlin_engine::json::Project = serde_json::from_value(v)
                    .context("failed to parse native Simlin JSON")
                    .map_err(AccessError::ParseError)?;
                (json_project.into(), SourceFormat::NativeJson)
            } else if v.get("variables").is_some() {
                let sdai_model: simlin_engine::json_sdai::SdaiModel = serde_json::from_value(v)
                    .context("failed to parse SD-AI JSON")
                    .map_err(AccessError::ParseError)?;
                (sdai_model.into(), SourceFormat::SdaiJson)
            } else {
                return Err(AccessError::ParseError(anyhow::anyhow!(
                    "unrecognized JSON format: expected top-level 'models' (native) or 'variables' (SD-AI)"
                )));
            }
        }
    };

    ensure_variable_uids(&mut project);
    Ok((project, format))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- AC7.1: native JSON detection ----

    #[test]
    fn ac7_1_open_project_detects_native_json() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic-growth.sd.json"
        ));
        let contents = std::fs::read_to_string(path).unwrap();
        let (project, format) = open_project(path, &contents).unwrap();
        assert_eq!(format, SourceFormat::NativeJson);
        assert!(
            !project.models.is_empty(),
            "project must have at least one model"
        );
    }

    // ---- AC7.2: SD-AI JSON detection ----

    #[test]
    fn ac7_2_open_project_detects_sdai_json() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sd-ai-simple.sd.json"
        ));
        let contents = std::fs::read_to_string(path).unwrap();
        let (project, format) = open_project(path, &contents).unwrap();
        assert_eq!(format, SourceFormat::SdaiJson);
        assert!(
            !project.models.is_empty(),
            "project must have at least one model"
        );
    }

    // ---- AC7.4: unrecognized JSON format returns descriptive error ----

    #[test]
    fn ac7_4_unrecognized_json_returns_error() {
        let path = std::path::Path::new("test.sd.json");
        let contents = r#"{"foo": "bar"}"#;
        let result = open_project(path, contents);
        assert!(result.is_err());
        let err = result.unwrap_err();
        // The variant must be ParseError so the rmcp tool layer can
        // distinguish bad bytes from missing files.
        assert!(matches!(err, AccessError::ParseError(_)));
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("models") && err_msg.contains("variables"),
            "error must mention expected formats: {err_msg}"
        );
    }

    // ---- AC7.5: .sd.json extension works for both formats ----

    #[test]
    fn ac7_5_sd_json_extension_works_for_both_formats() {
        let native_path = std::path::Path::new("model.sd.json");
        let native_content = r#"{"name":"test","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        let (_, format) = open_project(native_path, native_content).unwrap();
        assert_eq!(format, SourceFormat::NativeJson);

        let sdai_path = std::path::Path::new("model.sd.json");
        let sdai_content = r#"{"variables":[{"type":"variable","name":"x","equation":"1"}]}"#;
        let (_, format) = open_project(sdai_path, sdai_content).unwrap();
        assert_eq!(format, SourceFormat::SdaiJson);
    }

    // ---- XMILE detection ----

    #[test]
    fn open_project_detects_xmile() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        ));
        let contents = std::fs::read_to_string(path).unwrap();
        let (project, format) = open_project(path, &contents).unwrap();
        assert_eq!(format, SourceFormat::Xmile);
        assert!(!project.models.is_empty());
    }

    // ---- Vensim detection ----

    #[test]
    fn open_project_detects_mdl_as_its_own_format() {
        // `.mdl` must round-trip through the MDL writer, not the XMILE one,
        // so the opener has to report the format the saver needs.
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/test-models/samples/teacup/teacup.mdl"
        ));
        let contents = std::fs::read_to_string(path).unwrap();
        let (project, format) = open_project(path, &contents).unwrap();
        assert_eq!(format, SourceFormat::Mdl);
        assert!(!project.models.is_empty());
    }

    /// The extension table has three arms (XMILE-family, Vensim, and
    /// content-detected) and is case-insensitive; every arm is pinned here
    /// because `create_model` and `open_project` both dispatch on it.
    #[test]
    fn format_for_extension_covers_every_arm_case_insensitively() {
        let cases: &[(&str, Option<SourceFormat>)] = &[
            ("m.stmx", Some(SourceFormat::Xmile)),
            ("m.STMX", Some(SourceFormat::Xmile)),
            ("m.xmile", Some(SourceFormat::Xmile)),
            ("m.xml", Some(SourceFormat::Xmile)),
            ("m.mdl", Some(SourceFormat::Mdl)),
            ("m.MDL", Some(SourceFormat::Mdl)),
            ("m.sd.json", None),
            ("m.json", None),
            ("m", None),
        ];
        for (name, expected) in cases {
            assert_eq!(
                format_for_extension(std::path::Path::new(name)),
                *expected,
                "format_for_extension({name})"
            );
        }
    }

    // ---- Issue 1: ensure_variable_uids assigns UIDs on open ----

    // SD-AI JSON has no UIDs on variables. After open_project every variable
    // must have a UID so that SetLoopName (which maps variable names to UIDs)
    // can succeed without a "has no UID" error.
    #[test]
    fn open_project_sdai_assigns_uids_to_all_variables() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sd-ai-simple.sd.json"
        ));
        let contents = std::fs::read_to_string(path).unwrap();
        let (project, _) = open_project(path, &contents).unwrap();

        for model in &project.models {
            for var in &model.variables {
                let uid = match var {
                    simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                    simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                    simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                    simlin_engine::datamodel::Variable::Module(m) => m.uid,
                };
                assert!(
                    uid.is_some(),
                    "variable '{}' must have a UID after open_project",
                    var.get_ident()
                );
            }
        }
    }

    // Verifies that UIDs assigned by ensure_variable_uids are unique across
    // variables within each model.
    #[test]
    fn open_project_sdai_uids_are_unique() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sd-ai-simple.sd.json"
        ));
        let contents = std::fs::read_to_string(path).unwrap();
        let (project, _) = open_project(path, &contents).unwrap();

        for model in &project.models {
            let uids: Vec<i32> = model
                .variables
                .iter()
                .filter_map(|v| match v {
                    simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                    simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                    simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                    simlin_engine::datamodel::Variable::Module(m) => m.uid,
                })
                .collect();
            let unique: std::collections::HashSet<i32> = uids.iter().copied().collect();
            assert_eq!(
                uids.len(),
                unique.len(),
                "model '{}' must have unique UIDs across all variables",
                model.name
            );
        }
    }

    fn variable_uid(var: &simlin_engine::datamodel::Variable) -> Option<i32> {
        match var {
            simlin_engine::datamodel::Variable::Stock(s) => s.uid,
            simlin_engine::datamodel::Variable::Flow(f) => f.uid,
            simlin_engine::datamodel::Variable::Aux(a) => a.uid,
            simlin_engine::datamodel::Variable::Module(m) => m.uid,
        }
    }

    /// `ensure_variable_uids` must leave a UID the file already carries
    /// alone, and must draw fresh UIDs from a high-water mark that spans
    /// BOTH the variable UIDs and the view-element UIDs -- a fresh UID that
    /// collided with a view element would make `loop_metadata` ambiguous.
    ///
    /// The fixture is a native-JSON string rather than a hand-built
    /// `datamodel::Project` so the input arrives through the same parser
    /// production uses; it deliberately mixes a variable with a UID, one
    /// without, and a view element numbered ABOVE the explicit variable UID,
    /// which is what makes the two halves of the high-water mark separable.
    #[test]
    fn open_project_preserves_existing_uids_and_fills_above_the_view_high_water_mark() {
        let contents = r#"{
            "name": "uids",
            "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
            "models": [{
                "name": "main",
                "stocks": [{"uid": 7, "name": "population", "initialEquation": "100",
                            "inflows": ["births"], "outflows": []}],
                "flows": [{"name": "births", "equation": "population * 0.1"}],
                "auxiliaries": [{"name": "rate", "equation": "0.1"}],
                "views": [{"kind": "stock_flow", "elements": [
                    {"type": "aux", "uid": 13, "name": "rate", "x": 0, "y": 0}
                ], "zoom": 1}]
            }]
        }"#;
        let path = std::path::Path::new("uids.sd.json");
        let (project, format) = open_project(path, contents).unwrap();
        assert_eq!(format, SourceFormat::NativeJson);

        let model = &project.models[0];
        let by_name: std::collections::HashMap<&str, Option<i32>> = model
            .variables
            .iter()
            .map(|v| (v.get_ident(), variable_uid(v)))
            .collect();

        assert_eq!(
            by_name["population"],
            Some(7),
            "a UID present in the file must survive the open unchanged"
        );

        // The two UID-less variables are assigned above the view-element
        // high-water mark (13), not above the variable one (7).
        for name in ["births", "rate"] {
            let uid = by_name[name].expect("every variable must have a UID after open");
            assert!(
                uid > 13,
                "assigned UID for '{name}' must clear the view-element \
                 high-water mark of 13, got {uid}"
            );
        }

        let uids: Vec<i32> = model.variables.iter().filter_map(variable_uid).collect();
        let unique: std::collections::HashSet<i32> = uids.iter().copied().collect();
        assert_eq!(
            uids.len(),
            unique.len(),
            "UIDs must be unique within a model"
        );
    }

    // UIDs assigned on first open of an SD-AI file must survive a
    // serialize-then-reopen cycle.  Without persisting the uid field in
    // StockFields/FlowFields/AuxiliaryFields, each open regenerates UIDs from
    // the high-water-mark, which shifts if view elements are added between
    // saves and causes loop_metadata UIDs to point to wrong variables.
    #[test]
    fn open_project_sdai_uids_stable_across_serialize_reopen() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sd-ai-simple.sd.json"
        ));
        let contents = std::fs::read_to_string(path).unwrap();

        // First open: UIDs are assigned by ensure_variable_uids
        let (project1, _) = open_project(path, &contents).unwrap();
        let uids1: Vec<Option<i32>> = project1.models[0]
            .variables
            .iter()
            .map(|v| match v {
                simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                simlin_engine::datamodel::Variable::Module(m) => m.uid,
            })
            .collect();

        // Simulate save: convert to SdaiModel (which now embeds the uids) and
        // serialize back to JSON.
        let sdai_model = simlin_engine::json_sdai::SdaiModel::from(project1);
        let saved_json =
            serde_json::to_string_pretty(&sdai_model).expect("serialization must succeed");

        // Second open: UIDs should be read from the file, not reassigned
        let fake_path = std::path::Path::new("model.sd.json");
        let (project2, format2) = open_project(fake_path, &saved_json).unwrap();
        assert_eq!(format2, SourceFormat::SdaiJson);

        let uids2: Vec<Option<i32>> = project2.models[0]
            .variables
            .iter()
            .map(|v| match v {
                simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                simlin_engine::datamodel::Variable::Module(m) => m.uid,
            })
            .collect();

        assert_eq!(
            uids1, uids2,
            "UIDs must be identical after serialize-then-reopen"
        );
    }

    // A loop name set via loop_metadata UIDs must still resolve correctly after
    // the model has been saved and reopened.  This exercises the full scenario:
    //
    // 1. Open SD-AI file (UIDs assigned)
    // 2. Record the UIDs of two variables
    // 3. Attach loop_metadata referencing those UIDs
    // 4. Serialize to SD-AI JSON (UIDs now written to each variable's `uid` field)
    // 5. Reopen: UIDs are read back, not regenerated
    // 6. Loop metadata UIDs still map to the same variables
    #[test]
    fn loop_metadata_uids_remain_valid_after_save_reopen() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sd-ai-simple.sd.json"
        ));
        let contents = std::fs::read_to_string(path).unwrap();

        // Open and collect UIDs for loop metadata construction
        let (mut project, _) = open_project(path, &contents).unwrap();
        let loop_uids: Vec<i32> = project.models[0]
            .variables
            .iter()
            .filter_map(|v| match v {
                simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                simlin_engine::datamodel::Variable::Module(m) => m.uid,
            })
            .take(2)
            .collect();
        assert_eq!(loop_uids.len(), 2, "fixture must have at least 2 variables");

        // Attach loop metadata using those UIDs
        project.models[0]
            .loop_metadata
            .push(simlin_engine::datamodel::LoopMetadata {
                uids: loop_uids.clone(),
                deleted: false,
                name: "Test Loop".to_string(),
                description: "stability test".to_string(),
            });

        // Save: serialize to SD-AI JSON with embedded UIDs
        let sdai_model = simlin_engine::json_sdai::SdaiModel::from(project);
        let saved_json =
            serde_json::to_string_pretty(&sdai_model).expect("serialization must succeed");

        // Reopen: UIDs must match
        let fake_path = std::path::Path::new("model.sd.json");
        let (project2, _) = open_project(fake_path, &saved_json).unwrap();

        let uids_after: Vec<i32> = project2.models[0]
            .variables
            .iter()
            .filter_map(|v| match v {
                simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                simlin_engine::datamodel::Variable::Module(m) => m.uid,
            })
            .take(2)
            .collect();

        // The loop metadata UIDs stored in the file must still match the
        // variable UIDs present after reopening.
        assert_eq!(
            loop_uids, uids_after,
            "variable UIDs must be stable so loop_metadata references remain valid"
        );

        // The loop name must survive the roundtrip
        assert_eq!(project2.models[0].loop_metadata.len(), 1);
        assert_eq!(project2.models[0].loop_metadata[0].name, "Test Loop");
        assert_eq!(
            project2.models[0].loop_metadata[0].uids, loop_uids,
            "loop_metadata UIDs must match variable UIDs after reopen"
        );
    }
}
