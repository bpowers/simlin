// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP tool implementations for Simlin.
//!
//! Three tools are exposed:
//!
//! - `ReadModel`: Read a model file and return its JSON representation.
//! - `EditModel`: Apply a patch to an existing model file.
//! - `CreateModel`: Create a new empty model file.

mod create_model;
mod edit_model;
mod read_model;
pub mod types;

use std::io::BufReader;
use std::path::Path;

use anyhow::Context as _;

use crate::tool::Registry;

/// Register all Simlin MCP tools in the given registry.
pub fn register_all(registry: &mut Registry) {
    registry.register(Box::new(read_model::tool()));
    registry.register(Box::new(edit_model::tool()));
    registry.register(Box::new(create_model::tool()));
}

/// Resolve the model name to use, falling back to the first model when the
/// requested name is "main" and no model is literally named "main".
///
/// This allows tools to use "main" as a default that works for both
/// projects with a model named "main" and single-model projects imported
/// from XMILE or Vensim where the model may have a different name.
pub(crate) fn resolve_model_name<'a>(
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

/// Identifies how a model file was parsed, so write-back can use the
/// same format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceFormat {
    Xmile,
    NativeJson,
    SdaiJson,
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

/// Open a project from file contents.  XMILE and Vensim formats are
/// detected by extension; JSON files use content-based detection
/// (top-level `models` key = native, `variables` key = SD-AI).
pub(crate) fn open_project(
    path: &Path,
    contents: &str,
) -> anyhow::Result<(simlin_engine::datamodel::Project, SourceFormat)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (mut project, format) = match ext.as_str() {
        "stmx" | "xmile" | "xml" => {
            let mut reader = BufReader::new(contents.as_bytes());
            let project = simlin_engine::open_xmile(&mut reader)
                .map_err(|e| anyhow::anyhow!("failed to parse XMILE: {e:?}"))?;
            (project, SourceFormat::Xmile)
        }
        "mdl" => {
            let project = simlin_engine::open_vensim(contents)
                .map_err(|e| anyhow::anyhow!("failed to parse Vensim: {e:?}"))?;
            (project, SourceFormat::Xmile)
        }
        _ => {
            let v: serde_json::Value =
                serde_json::from_str(contents).context("failed to parse JSON")?;
            if v.get("models").is_some() {
                let json_project: simlin_engine::json::Project =
                    serde_json::from_value(v).context("failed to parse native Simlin JSON")?;
                (json_project.into(), SourceFormat::NativeJson)
            } else if v.get("variables").is_some() {
                let sdai_model: simlin_engine::json_sdai::SdaiModel =
                    serde_json::from_value(v).context("failed to parse SD-AI JSON")?;
                (sdai_model.into(), SourceFormat::SdaiJson)
            } else {
                anyhow::bail!(
                    "unrecognized JSON format: expected top-level 'models' (native) or 'variables' (SD-AI)"
                )
            }
        }
    };

    ensure_variable_uids(&mut project);
    Ok((project, format))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_all() {
        let mut registry = Registry::new();
        register_all(&mut registry);

        assert!(registry.get("ReadModel").is_some());
        assert!(registry.get("EditModel").is_some());
        assert!(registry.get("CreateModel").is_some());

        let defs = registry.definitions();
        assert_eq!(defs.len(), 3);
    }

    #[test]
    fn test_all_tools_have_valid_schemas() {
        let mut registry = Registry::new();
        register_all(&mut registry);

        for def in registry.definitions() {
            assert!(
                def.input_schema.is_object(),
                "tool {} should have an object schema",
                def.name
            );
            assert_eq!(
                def.input_schema["type"], "object",
                "tool {} schema type should be 'object'",
                def.name
            );
            assert!(
                def.input_schema["properties"].is_object(),
                "tool {} should have properties",
                def.name
            );
        }
    }

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
        let err_msg = result.unwrap_err().to_string();
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

    // Existing UIDs in a native JSON file must not be altered by ensure_variable_uids.
    #[test]
    fn open_project_native_json_preserves_existing_uids() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic-growth.sd.json"
        ));
        let contents = std::fs::read_to_string(path).unwrap();
        let (project, _) = open_project(path, &contents).unwrap();

        // Collect UIDs from a second open for comparison; both must match.
        let (project2, _) = open_project(path, &contents).unwrap();
        for (m1, m2) in project.models.iter().zip(project2.models.iter()) {
            for (v1, v2) in m1.variables.iter().zip(m2.variables.iter()) {
                let uid1 = match v1 {
                    simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                    simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                    simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                    simlin_engine::datamodel::Variable::Module(m) => m.uid,
                };
                let uid2 = match v2 {
                    simlin_engine::datamodel::Variable::Stock(s) => s.uid,
                    simlin_engine::datamodel::Variable::Flow(f) => f.uid,
                    simlin_engine::datamodel::Variable::Aux(a) => a.uid,
                    simlin_engine::datamodel::Variable::Module(m) => m.uid,
                };
                assert_eq!(uid1, uid2, "UIDs must be stable across repeated opens");
            }
        }
    }
}
