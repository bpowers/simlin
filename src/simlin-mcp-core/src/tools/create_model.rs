// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! Async `CreateModel` library function.
//!
//! Builds an empty `datamodel::Project` from the input and asks
//! [`ProjectAccess`] to persist it.  The `ProjectAccess::create` impl
//! decides where the bytes land (filesystem path or registry key) and
//! how they are serialised; this function only owns the in-memory
//! construction.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json as ejson;

use crate::access::ProjectAccess;
use crate::errors::AccessError;
use crate::open::format_for_extension;
use crate::types::{SourceFormat, build_empty_project_with_specs};

/// Input for the `CreateModel` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateModelInput {
    /// Path where the new model file should be created.  The extension
    /// picks the on-disk format: `.stmx`/`.xmile`/`.xml` write XMILE, `.mdl`
    /// writes Vensim, and anything else (`.sd.json` is conventional) writes
    /// Simlin JSON.
    pub project_path: String,

    /// Optional simulation specifications.  If omitted, defaults are
    /// used (start=0, end=100, dt=0.25, save_step=1, euler method).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_specs: Option<ejson::SimSpecs>,
}

/// Output from the `CreateModel` tool.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateModelOutput {
    pub project_path: String,
    pub sim_specs: ejson::SimSpecs,
    pub model_name: String,
}

/// Derive the project name from the filename stem, stripping the
/// `.sd.json` double-extension when present and any single extension
/// (`.json`, `.stmx`, `.mdl`, ...) otherwise.
fn project_name_from_path(path: &Path) -> String {
    let from_sd_json = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".sd.json"));
    match from_sd_json {
        Some(stem) => stem.to_string(),
        None => path
            .file_stem()
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "project".to_string()),
    }
}

/// Sim-spec defaults shared with the HTTP `POST /api/projects/new`
/// endpoint via [`crate::types::build_empty_project`].
fn default_sim_specs_for_create() -> ejson::SimSpecs {
    ejson::SimSpecs {
        start_time: 0.0,
        end_time: 100.0,
        dt: "0.25".to_string(),
        save_step: 1.0,
        method: "euler".to_string(),
        time_units: String::new(),
    }
}

/// Create an empty model at the given path.
///
/// CreateModel always produces a single `main` model with the requested
/// (or default) sim-specs and no variables.  The `ProjectAccess::create`
/// impl is responsible for refusing to overwrite an existing project
/// and for writing in the SourceFormat we tell it, which follows the
/// path's extension (`format_for_extension`) so the file `ReadModel`
/// later parses by extension holds the matching format; JSON is the
/// default for `.sd.json` and any other extension.
///
/// The empty project body is built via
/// [`crate::types::build_empty_project_with_specs`], the same helper the
/// HTTP `POST /api/projects/new` endpoint goes through.  Both paths
/// produce byte-identical files when invoked with default sim-specs and
/// matching path stems; Phase 8's parity test locks that property.
pub async fn create_model<A: ProjectAccess>(
    access: &A,
    input: CreateModelInput,
) -> Result<CreateModelOutput, AccessError> {
    let path = Path::new(&input.project_path);

    let sim_specs = input.sim_specs.unwrap_or_else(default_sim_specs_for_create);
    let project_name = project_name_from_path(path);
    let model_name = "main".to_string();

    let mut project = build_empty_project_with_specs(sim_specs.clone());
    project.name = project_name;

    let format = format_for_extension(path).unwrap_or(SourceFormat::NativeJson);
    access.create(path, &project, format).await?;

    Ok(CreateModelOutput {
        project_path: input.project_path,
        sim_specs,
        model_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name is derived from the file name in three shapes: the
    /// `.sd.json` double extension, a single extension of any kind, and a
    /// bare name.
    #[test]
    fn project_name_from_path_strips_each_extension_shape() {
        let cases = [
            ("dir/pop.sd.json", "pop"),
            ("dir/pop.json", "pop"),
            ("dir/pop.stmx", "pop"),
            ("dir/pop.mdl", "pop"),
            ("dir/pop", "pop"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                project_name_from_path(Path::new(input)),
                expected,
                "project_name_from_path({input})"
            );
        }
    }
}
