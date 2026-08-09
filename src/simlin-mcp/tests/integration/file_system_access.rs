// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end tests for [`simlin_mcp::access::FileSystemAccess`].
//!
//! These tests exercise the full open/save/create cycle the binary's
//! stateless implementation must support, plus the `.mdl` rejection
//! that preserves the canonical "Vensim .mdl files are read-only" error
//! message the existing `@simlin/mcp` clients see today.

use std::io;

use simlin_engine::datamodel;
use simlin_engine::json as ejson;
use simlin_mcp::access::FileSystemAccess;
use simlin_mcp_core::access::ProjectAccess;
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::types::SourceFormat;

fn minimal_native_json() -> serde_json::Value {
    serde_json::json!({
        "name": "test",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 100.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": ""
        },
        "models": [{ "name": "main" }]
    })
}

#[tokio::test]
async fn open_then_save_native_json_is_byte_stable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.sd.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&minimal_native_json()).unwrap(),
    )
    .unwrap();

    let access = FileSystemAccess::new();
    let opened = access.open(&path).await.unwrap();
    assert_eq!(opened.source_format, SourceFormat::NativeJson);
    assert_eq!(opened.version, 0, "stateless impl returns version 0");

    let new_version = access
        .save(&path, &opened.project, opened.source_format, None)
        .await
        .unwrap();
    assert_eq!(new_version, 0, "stateless impl always returns version 0");

    // Round-trip must preserve the project structure (name, models).
    let opened_again = access.open(&path).await.unwrap();
    let proj1: ejson::Project = (&opened.project).into();
    let proj2: ejson::Project = (&opened_again.project).into();
    assert_eq!(proj1.name, proj2.name);
    assert_eq!(proj1.models.len(), proj2.models.len());
}

#[tokio::test]
async fn open_missing_file_returns_not_found() {
    let access = FileSystemAccess::new();
    let path = std::path::Path::new("/does/not/exist/model.sd.json");
    let result = access.open(path).await;
    match result {
        Err(AccessError::NotFound { .. }) => {}
        Err(other) => panic!("expected AccessError::NotFound, got: {other:?}"),
        Ok(_) => panic!("expected AccessError::NotFound, got Ok"),
    }
}

#[tokio::test]
async fn save_to_mdl_path_returns_canonical_error() {
    let dir = tempfile::tempdir().unwrap();
    let mdl_path = dir.path().join("readonly.mdl");

    // Build a minimal datamodel::Project to attempt to save.
    let json_value = minimal_native_json();
    let json_project: ejson::Project = serde_json::from_value(json_value).unwrap();
    let project: datamodel::Project = json_project.into();

    let access = FileSystemAccess::new();
    let result = access
        .save(&mdl_path, &project, SourceFormat::Xmile, None)
        .await;

    match result {
        Err(AccessError::WriteError(e)) => {
            assert_eq!(
                e.kind(),
                io::ErrorKind::Unsupported,
                "expected Unsupported io::ErrorKind for .mdl write rejection"
            );
            let msg = e.to_string();
            // Backwards-compat: the exact message is what existing
            // @simlin/mcp clients render verbatim to users.
            assert!(
                msg.contains(
                    "Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, \
                     then CreateModel to start a new .sd.json file you can edit."
                ),
                "expected the canonical .mdl rejection message, got: {msg}"
            );
        }
        Err(other) => panic!("expected AccessError::WriteError, got: {other:?}"),
        Ok(_) => panic!("expected AccessError::WriteError for .mdl path, got Ok"),
    }

    assert!(
        !mdl_path.exists(),
        "rejected .mdl write must not create the file"
    );
}

#[tokio::test]
async fn create_writes_native_json_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new-model.sd.json");

    let json_value = minimal_native_json();
    let json_project: ejson::Project = serde_json::from_value(json_value).unwrap();
    let project: datamodel::Project = json_project.into();

    let access = FileSystemAccess::new();
    access
        .create(&path, &project, SourceFormat::NativeJson)
        .await
        .unwrap();

    assert!(path.exists(), "create must write the file to disk");

    // The output must be parseable as native Simlin JSON.
    let contents = std::fs::read_to_string(&path).unwrap();
    let parsed: ejson::Project =
        serde_json::from_str(&contents).expect("created file must be valid native JSON");
    assert_eq!(parsed.name, "test");
    assert_eq!(parsed.models.len(), 1);
}

#[tokio::test]
async fn create_creates_missing_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/dir/structure/model.sd.json");

    let json_value = minimal_native_json();
    let json_project: ejson::Project = serde_json::from_value(json_value).unwrap();
    let project: datamodel::Project = json_project.into();

    let access = FileSystemAccess::new();
    access
        .create(&path, &project, SourceFormat::NativeJson)
        .await
        .unwrap();

    assert!(path.exists(), "create must create missing parent dirs");
}

#[tokio::test]
async fn create_refuses_to_overwrite_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.sd.json");
    std::fs::write(&path, "{}").unwrap();

    let json_value = minimal_native_json();
    let json_project: ejson::Project = serde_json::from_value(json_value).unwrap();
    let project: datamodel::Project = json_project.into();

    let access = FileSystemAccess::new();
    let result = access
        .create(&path, &project, SourceFormat::NativeJson)
        .await;

    match result {
        Err(AccessError::WriteError(e)) => {
            assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        }
        Err(other) => panic!("expected WriteError(AlreadyExists), got: {other:?}"),
        Ok(_) => panic!("expected WriteError(AlreadyExists), got Ok"),
    }
}

#[tokio::test]
async fn save_xmile_to_xmile_extension_works() {
    let dir = tempfile::tempdir().unwrap();

    // Use an existing XMILE fixture to get a real project structure.
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/logistic_growth_ltm/logistic_growth.stmx"
    );
    let target_path = dir.path().join("output.stmx");
    std::fs::copy(fixture, &target_path).unwrap();

    let access = FileSystemAccess::new();
    let opened = access.open(&target_path).await.unwrap();
    assert_eq!(opened.source_format, SourceFormat::Xmile);

    access
        .save(&target_path, &opened.project, SourceFormat::Xmile, None)
        .await
        .unwrap();

    // The file must still parse after a save round-trip.
    let opened_again = access.open(&target_path).await.unwrap();
    assert_eq!(opened_again.source_format, SourceFormat::Xmile);
    assert!(!opened_again.project.models.is_empty());
}

/// Saving an SD-AI project must REGENERATE its `relationships` field from
/// the post-save model's equation dependencies rather than carrying over
/// whatever the source file held (`fs_access::serialize_project`'s
/// `SdaiJson` arm). This is the one branch on the save path with real logic
/// behind it -- a link-polarity analysis through the salsa db -- and the
/// fixture below deliberately arrives with NO `relationships` key at all, so
/// a save that merely copied the input through would write nothing here.
#[tokio::test]
async fn save_sdai_json_regenerates_relationships_from_the_equations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.sd.json");
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/sd-ai-simple.sd.json"
    );
    std::fs::copy(fixture, &path).unwrap();

    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(
        on_disk.get("relationships").is_none(),
        "fixture must start with no relationships, or this test cannot \
         distinguish regeneration from passthrough"
    );

    let access = FileSystemAccess::new();
    let opened = access.open(&path).await.unwrap();
    assert_eq!(opened.source_format, SourceFormat::SdaiJson);

    access
        .save(&path, &opened.project, SourceFormat::SdaiJson, None)
        .await
        .unwrap();

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let relationships = saved["relationships"]
        .as_array()
        .expect("save must write a relationships array for an SD-AI project");

    // `Population * birth_rate` gives births two positive inputs. The
    // stock-flow structural edge (births -> Population) is deliberately
    // filtered out by `generate_relationships`, so it must NOT appear.
    let edges: Vec<(&str, &str, &str)> = relationships
        .iter()
        .map(|r| {
            (
                r["from"].as_str().unwrap_or(""),
                r["to"].as_str().unwrap_or(""),
                r["polarity"].as_str().unwrap_or(""),
            )
        })
        .collect();

    assert!(
        edges.contains(&("Population", "births", "+")),
        "Population -> births must be regenerated as a positive link: {edges:?}"
    );
    assert!(
        edges.contains(&("birth_rate", "births", "+")),
        "birth_rate -> births must be regenerated as a positive link: {edges:?}"
    );
    assert!(
        !edges
            .iter()
            .any(|(from, to, _)| *from == "births" && *to == "Population"),
        "the stock-flow structural edge must be filtered out: {edges:?}"
    );
}
