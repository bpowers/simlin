// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for JSON roundtrip using real XMILE models.
//!
//! These tests verify that:
//! 1. XMILE models can be converted to JSON and back through protobuf
//! 2. The roundtrip is idempotent after the first conversion
//!
//! This is critical for ensuring user models stored as protobuf can be safely
//! migrated to JSON format.

use std::fs::File;
use std::io::BufReader;

use simlin_engine::prost::Message;
use simlin_engine::xmile;
use simlin_engine::{datamodel, json, project_io, serde as project_serde};

/// Performs a full roundtrip through protobuf and JSON:
/// datamodel -> protobuf bytes -> datamodel -> JSON string
fn roundtrip_to_json(dm: &datamodel::Project) -> (Vec<u8>, String) {
    // datamodel -> protobuf -> bytes
    let pb: project_io::Project = project_serde::serialize(dm);
    let mut pb_bytes = Vec::new();
    pb.encode(&mut pb_bytes).unwrap();

    // bytes -> protobuf -> datamodel -> JSON
    let pb_decoded = project_io::Project::decode(&pb_bytes[..]).unwrap();
    let dm_decoded: datamodel::Project = project_serde::deserialize(pb_decoded);
    let json_project: json::Project = dm_decoded.into();
    let json_str = serde_json::to_string(&json_project).unwrap();

    (pb_bytes, json_str)
}

/// Performs a roundtrip from JSON string back through protobuf:
/// JSON string -> datamodel -> protobuf bytes -> datamodel -> JSON string
fn roundtrip_from_json(json_str: &str) -> (Vec<u8>, String) {
    let json_project: json::Project = serde_json::from_str(json_str).unwrap();
    let dm: datamodel::Project = json_project.into();
    roundtrip_to_json(&dm)
}

/// Tests that XMILE -> JSON roundtrip is idempotent after first conversion.
///
/// Due to:
/// - JSON having separate arrays (stocks, flows, auxiliaries) vs datamodel's single variables vec
/// - Floating point precision differences in JSON decimal representation
///
/// We verify that after the first roundtrip, subsequent roundtrips produce identical results.
fn test_xmile_json_roundtrip(xmile_path: &str) {
    let file_path = format!("../../{xmile_path}");
    eprintln!("testing JSON roundtrip: {xmile_path}");

    let f = File::open(&file_path).unwrap_or_else(|e| panic!("Failed to open {file_path}: {e}"));
    let mut f = BufReader::new(f);

    let datamodel_project = xmile::project_from_reader(&mut f)
        .unwrap_or_else(|e| panic!("Failed to parse {xmile_path}: {e}"));

    // First roundtrip: XMILE -> datamodel -> pb -> json
    let (_, json_str1) = roundtrip_to_json(&datamodel_project);

    // Second roundtrip: json -> datamodel -> pb -> json
    let (pb_bytes2, json_str2) = roundtrip_from_json(&json_str1);

    // Third roundtrip to verify idempotence
    let (pb_bytes3, json_str3) = roundtrip_from_json(&json_str2);

    // After first roundtrip, JSON should be stable
    assert_eq!(
        json_str2, json_str3,
        "JSON should be identical after second roundtrip for {xmile_path}"
    );

    // Protobuf bytes should also be stable
    assert_eq!(
        pb_bytes2, pb_bytes3,
        "Protobuf bytes should be identical after second roundtrip for {xmile_path}"
    );

    // Verify the JSON can be deserialized and has expected structure
    let parsed: json::Project = serde_json::from_str(&json_str2).unwrap();
    assert!(
        !parsed.models.is_empty(),
        "Parsed project should have at least one model for {xmile_path}"
    );
}

// Test models from the test suite - these cover a wide range of model features

#[test]
fn test_sir_model_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/samples/SIR/SIR.xmile");
}

#[test]
fn test_sir_reciprocal_dt_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/samples/SIR/SIR_reciprocal-dt.xmile");
}

#[test]
fn test_teacup_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/samples/teacup/teacup.xmile");
}

#[test]
fn test_teacup_with_diagram_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/samples/teacup/teacup_w_diagram.xmile");
}

#[test]
fn test_hares_and_lynxes_modules_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
    );
}

// Array models - test subscripted variables
#[test]
fn test_a2a_arrays_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/samples/arrays/a2a/a2a.stmx");
}

#[test]
fn test_non_a2a_arrays_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/samples/arrays/non-a2a/non-a2a.stmx");
}

#[test]
fn test_subscript_1d_arrays_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/tests/subscript_1d_arrays/test_subscript_1d_arrays.xmile",
    );
}

#[test]
fn test_subscript_2d_arrays_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
    );
}

#[test]
fn test_subscript_3d_arrays_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/tests/subscript_3d_arrays/test_subscript_3d_arrays.xmile",
    );
}

// Lookup/graphical function tests
#[test]
fn test_lookups_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/lookups_simlin/test_lookups.xmile");
}

#[test]
fn test_lookups_inline_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/lookups_inline/test_lookups_inline.xmile");
}

#[test]
fn test_lookups_inline_bounded_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/tests/lookups_inline_bounded/test_lookups_inline_bounded.xmile",
    );
}

#[test]
fn test_lookups_with_expr_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/tests/lookups_with_expr/test_lookups_with_expr.xmile",
    );
}

// Various equation features
#[test]
fn test_delays_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/delays2/delays.xmile");
}

#[test]
fn test_smooth_and_stock_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.xmile",
    );
}

#[test]
fn test_trend_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/trend/test_trend.xmile");
}

#[test]
fn test_if_stmt_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/if_stmt/if_stmt.xmile");
}

#[test]
fn test_logicals_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/logicals/test_logicals.xmile");
}

#[test]
fn test_comparisons_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/comparisons/comparisons.xmile");
}

// Math function tests
#[test]
fn test_trig_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/trig/test_trig.xmile");
}

#[test]
fn test_abs_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/abs/test_abs.xmile");
}

#[test]
fn test_sqrt_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/sqrt/test_sqrt.xmile");
}

#[test]
fn test_exp_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/exp/test_exp.xmile");
}

#[test]
fn test_ln_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/ln/test_ln.xmile");
}

#[test]
fn test_log_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/log/test_log.xmile");
}

#[test]
fn test_exponentiation_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/exponentiation/exponentiation.xmile");
}

// Edge cases and special handling
#[test]
fn test_unicode_characters_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/unicode_characters/unicode_test_model.xmile");
}

#[test]
fn test_model_doc_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/model_doc/model_doc.xmile");
}

#[test]
fn test_line_breaks_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/line_breaks/test_line_breaks.xmile");
}

#[test]
fn test_line_continuation_json_roundtrip() {
    test_xmile_json_roundtrip(
        "test/test-models/tests/line_continuation/test_line_continuation.xmile",
    );
}

#[test]
fn test_input_functions_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/input_functions/test_inputs.xmile");
}

#[test]
fn test_game_json_roundtrip() {
    test_xmile_json_roundtrip("test/test-models/tests/game/test_game.xmile");
}
