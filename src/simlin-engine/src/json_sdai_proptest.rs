// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property-based tests for SDAI JSON serialization using proptest.
//!
//! These tests verify that:
//! 1. JSON serialization roundtrips correctly (JSON -> Rust -> JSON -> Rust)
//! 2. Datamodel conversions roundtrip correctly (SDAI types <-> datamodel types)
//! 3. Generated JSON validates against the schema

use proptest::prelude::*;

use crate::datamodel;
use crate::json_sdai::*;

// Strategy helpers for generating valid identifiers and equations

fn ident_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}".prop_map(|s| s.to_string())
}

fn equation_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("0".to_string()),
        Just("1".to_string()),
        (1i32..1000).prop_map(|n| n.to_string()),
        (0.01f64..100.0).prop_map(|f| format!("{:.2}", f)),
        ident_strategy(),
        (ident_strategy(), ident_strategy()).prop_map(|(a, b)| format!("{} + {}", a, b)),
        (ident_strategy(), ident_strategy()).prop_map(|(a, b)| format!("{} * {}", a, b)),
    ]
}

fn finite_f64() -> impl Strategy<Value = f64> {
    // Generate floats that roundtrip correctly through JSON serialization.
    // We use integers and simple fractions to avoid precision loss.
    prop_oneof![
        Just(0.0),
        Just(1.0),
        Just(-1.0),
        (-1000i32..1000).prop_map(|x| x as f64),
        (-100i32..100).prop_map(|x| x as f64 / 10.0),
        (-100i32..100).prop_map(|x| x as f64 / 4.0),
    ]
}

fn documentation_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "[A-Za-z0-9 ]{0,50}".prop_map(|s| s.to_string()),
    ]
}

fn units_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("people".to_string()),
        Just("widgets/year".to_string()),
        Just("1/time".to_string()),
    ]
}

fn polarity_strategy() -> impl Strategy<Value = Polarity> {
    prop_oneof![
        Just(Polarity::Positive),
        Just(Polarity::Negative),
        Just(Polarity::Unknown),
    ]
}

// Leaf type strategies

fn point_strategy() -> impl Strategy<Value = Point> {
    (finite_f64(), finite_f64()).prop_map(|(x, y)| Point { x, y })
}

fn graphical_function_strategy() -> impl Strategy<Value = GraphicalFunction> {
    prop::collection::vec(point_strategy(), 2..6).prop_map(|points| GraphicalFunction { points })
}

/// Generate Option<Vec<T>> that is either None or Some(non-empty vec).
/// This avoids generating Some([]) which normalizes to None during roundtrip,
/// ensuring generated values are in canonical form.
fn option_non_empty_vec<S: Strategy>(
    element_strategy: S,
    max_len: usize,
) -> impl Strategy<Value = Option<Vec<S::Value>>>
where
    S::Value: std::fmt::Debug + Clone,
{
    prop_oneof![
        Just(None),
        prop::collection::vec(element_strategy, 1..=max_len).prop_map(Some),
    ]
}

fn stock_fields_strategy() -> impl Strategy<Value = StockFields> {
    (
        ident_strategy(),
        prop::option::of(equation_strategy()),
        prop::option::of(documentation_strategy()),
        prop::option::of(units_strategy()),
        option_non_empty_vec(ident_strategy(), 3),
        option_non_empty_vec(ident_strategy(), 3),
        prop::option::of(graphical_function_strategy()),
    )
        .prop_map(
            |(name, equation, documentation, units, inflows, outflows, graphical_function)| {
                StockFields {
                    name,
                    equation,
                    documentation,
                    units,
                    inflows,
                    outflows,
                    graphical_function,
                }
            },
        )
}

fn flow_fields_strategy() -> impl Strategy<Value = FlowFields> {
    (
        ident_strategy(),
        prop::option::of(equation_strategy()),
        prop::option::of(documentation_strategy()),
        prop::option::of(units_strategy()),
        prop::option::of(graphical_function_strategy()),
    )
        .prop_map(
            |(name, equation, documentation, units, graphical_function)| FlowFields {
                name,
                equation,
                documentation,
                units,
                graphical_function,
            },
        )
}

fn auxiliary_fields_strategy() -> impl Strategy<Value = AuxiliaryFields> {
    (
        ident_strategy(),
        prop::option::of(equation_strategy()),
        prop::option::of(documentation_strategy()),
        prop::option::of(units_strategy()),
        prop::option::of(graphical_function_strategy()),
    )
        .prop_map(
            |(name, equation, documentation, units, graphical_function)| AuxiliaryFields {
                name,
                equation,
                documentation,
                units,
                graphical_function,
            },
        )
}

fn variable_strategy() -> impl Strategy<Value = Variable> {
    prop_oneof![
        stock_fields_strategy().prop_map(Variable::Stock),
        flow_fields_strategy().prop_map(Variable::Flow),
        auxiliary_fields_strategy().prop_map(Variable::Variable),
    ]
}

fn relationship_strategy() -> impl Strategy<Value = Relationship> {
    (
        prop::option::of(documentation_strategy()),
        ident_strategy(),
        ident_strategy(),
        polarity_strategy(),
        prop::option::of(documentation_strategy()),
    )
        .prop_map(
            |(reasoning, from, to, polarity, polarity_reasoning)| Relationship {
                reasoning,
                from,
                to,
                polarity,
                polarity_reasoning,
            },
        )
}

fn sim_specs_strategy() -> impl Strategy<Value = SimSpecs> {
    (
        finite_f64(),
        finite_f64(),
        prop::option::of(prop_oneof![Just(0.25), Just(0.5), Just(1.0)]),
        prop::option::of(prop_oneof![
            Just("months".to_string()),
            Just("years".to_string()),
        ]),
        prop::option::of(prop_oneof![Just(1.0), Just(0.5)]),
        prop::option::of(prop_oneof![
            Just("euler".to_string()),
            Just("rk4".to_string()),
        ]),
    )
        .prop_map(|(start, stop, dt, time_units, save_step, method)| {
            let (start_time, stop_time) = if start <= stop {
                (start, stop)
            } else {
                (stop, start)
            };
            SimSpecs {
                start_time,
                stop_time,
                dt,
                time_units,
                save_step,
                method,
            }
        })
}

fn sdai_model_strategy() -> impl Strategy<Value = SdaiModel> {
    (
        prop::collection::vec(variable_strategy(), 0..5),
        prop::option::of(prop::collection::vec(relationship_strategy(), 0..3)),
        prop::option::of(sim_specs_strategy()),
    )
        .prop_map(|(variables, relationships, specs)| SdaiModel {
            variables,
            relationships,
            specs,
            views: None, // Views are complex and tested separately in json_proptest
        })
}

// Property tests

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // JSON serialization roundtrip tests

    #[test]
    fn json_roundtrip_point(point in point_strategy()) {
        let json = serde_json::to_string(&point).unwrap();
        let parsed: Point = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(point, parsed);
    }

    #[test]
    fn json_roundtrip_graphical_function(gf in graphical_function_strategy()) {
        let json = serde_json::to_string(&gf).unwrap();
        let parsed: GraphicalFunction = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(gf, parsed);
    }

    #[test]
    fn json_roundtrip_stock_fields(stock in stock_fields_strategy()) {
        let json = serde_json::to_string(&stock).unwrap();
        let parsed: StockFields = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(stock, parsed);
    }

    #[test]
    fn json_roundtrip_flow_fields(flow in flow_fields_strategy()) {
        let json = serde_json::to_string(&flow).unwrap();
        let parsed: FlowFields = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(flow, parsed);
    }

    #[test]
    fn json_roundtrip_auxiliary_fields(aux in auxiliary_fields_strategy()) {
        let json = serde_json::to_string(&aux).unwrap();
        let parsed: AuxiliaryFields = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(aux, parsed);
    }

    #[test]
    fn json_roundtrip_variable(var in variable_strategy()) {
        let json = serde_json::to_string(&var).unwrap();
        let parsed: Variable = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(var, parsed);
    }

    #[test]
    fn json_roundtrip_relationship(rel in relationship_strategy()) {
        let json = serde_json::to_string(&rel).unwrap();
        let parsed: Relationship = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(rel, parsed);
    }

    #[test]
    fn json_roundtrip_sim_specs(specs in sim_specs_strategy()) {
        let json = serde_json::to_string(&specs).unwrap();
        let parsed: SimSpecs = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(specs, parsed);
    }

    #[test]
    fn json_roundtrip_sdai_model(model in sdai_model_strategy()) {
        let json = serde_json::to_string(&model).unwrap();
        let parsed: SdaiModel = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(model, parsed);
    }

    // Datamodel conversion roundtrip tests

    #[test]
    fn datamodel_roundtrip_graphical_function(gf in graphical_function_strategy()) {
        let dm: datamodel::GraphicalFunction = gf.clone().into();
        let sdai_back: GraphicalFunction = dm.into();
        // Points should roundtrip
        prop_assert_eq!(gf.points.len(), sdai_back.points.len());
        for (p1, p2) in gf.points.iter().zip(sdai_back.points.iter()) {
            prop_assert!((p1.x - p2.x).abs() < 1e-10);
            prop_assert!((p1.y - p2.y).abs() < 1e-10);
        }
    }

    #[test]
    fn datamodel_roundtrip_stock_fields(stock in stock_fields_strategy()) {
        let dm: datamodel::Stock = stock.clone().into();
        let sdai_back: StockFields = dm.into();
        // Core fields should match exactly since strategy only generates canonical forms
        prop_assert_eq!(stock.name, sdai_back.name);
        prop_assert_eq!(stock.equation, sdai_back.equation);
        prop_assert_eq!(stock.inflows, sdai_back.inflows);
        prop_assert_eq!(stock.outflows, sdai_back.outflows);
    }

    #[test]
    fn datamodel_roundtrip_flow_fields(flow in flow_fields_strategy()) {
        let dm: datamodel::Flow = flow.clone().into();
        let sdai_back: FlowFields = dm.into();
        prop_assert_eq!(flow.name, sdai_back.name);
        prop_assert_eq!(flow.equation, sdai_back.equation);
    }

    #[test]
    fn datamodel_roundtrip_auxiliary_fields(aux in auxiliary_fields_strategy()) {
        let dm: datamodel::Aux = aux.clone().into();
        let sdai_back: AuxiliaryFields = dm.into();
        prop_assert_eq!(aux.name, sdai_back.name);
        prop_assert_eq!(aux.equation, sdai_back.equation);
    }

    #[test]
    fn datamodel_roundtrip_sim_specs(specs in sim_specs_strategy()) {
        let dm: datamodel::SimSpecs = specs.clone().into();
        let sdai_back: SimSpecs = dm.into();
        prop_assert_eq!(specs.start_time, sdai_back.start_time);
        prop_assert_eq!(specs.stop_time, sdai_back.stop_time);
        // dt roundtrips as Some(value)
        if specs.dt.is_some() {
            prop_assert!(sdai_back.dt.is_some());
        }
    }

    #[test]
    fn datamodel_roundtrip_sdai_model(model in sdai_model_strategy()) {
        let dm: datamodel::Project = model.clone().into();
        let sdai_back: SdaiModel = dm.into();
        // Variable count should match
        prop_assert_eq!(model.variables.len(), sdai_back.variables.len());
    }

}

#[cfg(feature = "schema")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // Schema validation tests

    #[test]
    fn generated_json_validates_against_schema(model in sdai_model_strategy()) {
        let json_value = serde_json::to_value(&model).unwrap();
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value)
            .expect("schema should be valid");
        prop_assert!(
            validator.is_valid(&json_value),
            "Generated JSON failed schema validation"
        );
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    /// This test writes to the file system to regenerate the schema file.
    /// It is marked #[ignore] to avoid file system writes during normal test runs.
    /// Run with `cargo test -- --ignored generate_and_write_sdai_schema` when needed.
    #[test]
    #[ignore]
    fn generate_and_write_sdai_schema() {
        let schema_json = generate_schema_json();
        let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("doc/sdai-model.schema.json");

        fs::write(&schema_path, &schema_json).expect("failed to write schema file");

        // Verify it's valid JSON and can be used as a schema
        let parsed: serde_json::Value = serde_json::from_str(&schema_json).unwrap();
        assert!(parsed.is_object());
        assert!(parsed.get("$schema").is_some());
    }

    #[test]
    fn sdai_schema_is_valid_json_schema() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();

        // Verify the schema can be compiled
        let result = jsonschema::validator_for(&schema_value);
        assert!(result.is_ok(), "Schema should be valid: {:?}", result.err());
    }

    #[test]
    fn test_variable_type_discriminator() {
        // Verify that the type field is correctly serialized
        let stock = Variable::Stock(StockFields {
            name: "inventory".to_string(),
            equation: Some("100".to_string()),
            documentation: None,
            units: None,
            inflows: None,
            outflows: None,
            graphical_function: None,
        });

        let json = serde_json::to_string(&stock).unwrap();
        assert!(
            json.contains("\"type\":\"stock\""),
            "Stock should have type discriminator: {}",
            json
        );

        let flow = Variable::Flow(FlowFields {
            name: "rate".to_string(),
            equation: Some("10".to_string()),
            documentation: None,
            units: None,
            graphical_function: None,
        });

        let json = serde_json::to_string(&flow).unwrap();
        assert!(
            json.contains("\"type\":\"flow\""),
            "Flow should have type discriminator: {}",
            json
        );

        let aux = Variable::Variable(AuxiliaryFields {
            name: "target".to_string(),
            equation: Some("50".to_string()),
            documentation: None,
            units: None,
            graphical_function: None,
        });

        let json = serde_json::to_string(&aux).unwrap();
        assert!(
            json.contains("\"type\":\"variable\""),
            "Auxiliary should have type discriminator: {}",
            json
        );
    }

    #[test]
    fn test_camel_case_serialization() {
        // Verify that camelCase field names are used
        let specs = SimSpecs {
            start_time: 0.0,
            stop_time: 100.0,
            dt: Some(0.25),
            time_units: Some("months".to_string()),
            save_step: Some(1.0),
            method: Some("rk4".to_string()),
        };

        let json = serde_json::to_string(&specs).unwrap();
        assert!(
            json.contains("\"startTime\""),
            "Should use camelCase startTime: {}",
            json
        );
        assert!(
            json.contains("\"stopTime\""),
            "Should use camelCase stopTime: {}",
            json
        );
        assert!(
            json.contains("\"timeUnits\""),
            "Should use camelCase timeUnits: {}",
            json
        );
        assert!(
            json.contains("\"saveStep\""),
            "Should use camelCase saveStep: {}",
            json
        );
        assert!(
            !json.contains("start_time"),
            "Should not use snake_case: {}",
            json
        );
    }

    // Negative test cases: Invalid JSON that should fail schema validation

    #[test]
    fn invalid_json_missing_required_field() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        // Missing required "variables" field
        let invalid_json: serde_json::Value = serde_json::json!({
            "specs": { "startTime": 0.0, "stopTime": 100.0 }
        });
        assert!(
            !validator.is_valid(&invalid_json),
            "JSON missing 'variables' should fail validation"
        );
    }

    #[test]
    fn invalid_json_wrong_type_for_field() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        // "variables" should be an array, not a string
        let invalid_json: serde_json::Value = serde_json::json!({
            "variables": "not an array"
        });
        assert!(
            !validator.is_valid(&invalid_json),
            "JSON with wrong type for 'variables' should fail validation"
        );
    }

    #[test]
    fn invalid_json_variable_missing_type_discriminator() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        // Variable without "type" field
        let invalid_json: serde_json::Value = serde_json::json!({
            "variables": [
                { "name": "inventory", "equation": "100" }
            ]
        });
        assert!(
            !validator.is_valid(&invalid_json),
            "Variable missing 'type' discriminator should fail validation"
        );
    }

    #[test]
    fn invalid_json_variable_wrong_type_discriminator() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        // Variable with invalid "type" value
        let invalid_json: serde_json::Value = serde_json::json!({
            "variables": [
                { "type": "invalid_type", "name": "inventory" }
            ]
        });
        assert!(
            !validator.is_valid(&invalid_json),
            "Variable with invalid 'type' should fail validation"
        );
    }

    #[test]
    fn invalid_json_relationship_missing_required_fields() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        // Relationship missing "from", "to", "polarity"
        let invalid_json: serde_json::Value = serde_json::json!({
            "variables": [],
            "relationships": [
                { "reasoning": "some reason" }
            ]
        });
        assert!(
            !validator.is_valid(&invalid_json),
            "Relationship missing required fields should fail validation"
        );
    }

    #[test]
    fn invalid_json_sim_specs_missing_required_fields() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        // SimSpecs missing required startTime/stopTime
        let invalid_json: serde_json::Value = serde_json::json!({
            "variables": [],
            "specs": { "dt": 0.5 }
        });
        assert!(
            !validator.is_valid(&invalid_json),
            "SimSpecs missing required fields should fail validation"
        );
    }

    // Edge case tests

    #[test]
    fn test_empty_points_graphical_function() {
        // Empty points array is valid JSON
        let gf = GraphicalFunction { points: vec![] };
        let json = serde_json::to_string(&gf).unwrap();
        let parsed: GraphicalFunction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.points.len(), 0);

        // Verify it validates against schema
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        let model_with_empty_gf: serde_json::Value = serde_json::json!({
            "variables": [{
                "type": "flow",
                "name": "rate",
                "graphicalFunction": { "points": [] }
            }]
        });
        assert!(
            validator.is_valid(&model_with_empty_gf),
            "Empty points array should be valid JSON"
        );
    }

    #[test]
    fn test_empty_graphical_function_produces_valid_scales() {
        // Empty points should produce valid 0-1 default scales, not INFINITY
        let gf = GraphicalFunction { points: vec![] };
        let dm_gf: datamodel::GraphicalFunction = gf.into();

        // Verify scales are valid (0-1 default), not INFINITY
        assert!(
            dm_gf.x_scale.min.is_finite(),
            "x_scale.min should be finite, got {}",
            dm_gf.x_scale.min
        );
        assert!(
            dm_gf.x_scale.max.is_finite(),
            "x_scale.max should be finite, got {}",
            dm_gf.x_scale.max
        );
        assert!(
            dm_gf.y_scale.min.is_finite(),
            "y_scale.min should be finite, got {}",
            dm_gf.y_scale.min
        );
        assert!(
            dm_gf.y_scale.max.is_finite(),
            "y_scale.max should be finite, got {}",
            dm_gf.y_scale.max
        );

        // Verify default scale is 0-1
        assert_eq!(dm_gf.x_scale.min, 0.0);
        assert_eq!(dm_gf.x_scale.max, 1.0);
        assert_eq!(dm_gf.y_scale.min, 0.0);
        assert_eq!(dm_gf.y_scale.max, 1.0);
    }

    #[test]
    fn test_single_point_graphical_function() {
        // Single point is technically valid but unusual
        let gf = GraphicalFunction {
            points: vec![Point { x: 0.0, y: 1.0 }],
        };
        let json = serde_json::to_string(&gf).unwrap();
        let parsed: GraphicalFunction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.points.len(), 1);
    }

    #[test]
    fn test_polarity_values_in_relationship() {
        // Valid polarity values with their JSON representations
        let test_cases = [
            (Polarity::Positive, "+"),
            (Polarity::Negative, "-"),
            (Polarity::Unknown, "?"),
        ];

        for (polarity, expected_str) in test_cases {
            let rel = Relationship {
                reasoning: None,
                from: "a".to_string(),
                to: "b".to_string(),
                polarity,
                polarity_reasoning: None,
            };
            let json = serde_json::to_string(&rel).unwrap();
            assert!(
                json.contains(&format!("\"polarity\":\"{}\"", expected_str)),
                "Polarity {:?} should serialize to '{}': {}",
                polarity,
                expected_str,
                json
            );
            let parsed: Relationship = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.polarity, polarity);
        }
    }

    #[test]
    fn invalid_json_polarity_value() {
        // Invalid polarity values should fail to parse
        let invalid_json = r#"{"from": "a", "to": "b", "polarity": "invalid"}"#;
        let result: Result<Relationship, _> = serde_json::from_str(invalid_json);
        assert!(
            result.is_err(),
            "Invalid polarity value should fail to parse"
        );

        // Also test schema validation
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();

        let invalid_model: serde_json::Value = serde_json::json!({
            "variables": [],
            "relationships": [
                { "from": "a", "to": "b", "polarity": "invalid" }
            ]
        });
        assert!(
            !validator.is_valid(&invalid_model),
            "Invalid polarity value should fail schema validation"
        );
    }

    #[test]
    fn test_empty_model() {
        let empty_model = SdaiModel {
            variables: vec![],
            relationships: None,
            specs: None,
            views: None,
        };

        let json = serde_json::to_string(&empty_model).unwrap();
        let parsed: SdaiModel = serde_json::from_str(&json).unwrap();
        assert!(parsed.variables.is_empty());
        assert!(parsed.relationships.is_none());

        // Verify against schema
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value).unwrap();
        let json_value = serde_json::to_value(&empty_model).unwrap();
        assert!(
            validator.is_valid(&json_value),
            "Empty model should be valid"
        );
    }
}

#[cfg(test)]
mod protobuf_roundtrip_tests {
    use super::*;
    use crate::project_io;
    use crate::prost::Message;
    use crate::serde as project_serde;

    /// Performs a full protobuf -> JSON roundtrip for SDAI models:
    /// 1. Converts SdaiModel to datamodel::Project
    /// 2. Converts datamodel to project_io::Project (protobuf)
    /// 3. Encodes to protobuf bytes
    /// 4. Decodes protobuf bytes back
    /// 5. Converts through datamodel back to SdaiModel
    /// 6. Serializes to JSON string
    fn roundtrip_sdai_pb_json(sdai_model: &SdaiModel) -> (Vec<u8>, String) {
        // sdai -> datamodel -> protobuf
        let dm_project: datamodel::Project = sdai_model.clone().into();
        let pb_project: project_io::Project = project_serde::serialize(&dm_project);

        // Encode to protobuf bytes
        let mut pb_bytes = Vec::new();
        pb_project.encode(&mut pb_bytes).unwrap();

        // Decode protobuf bytes
        let pb_decoded = project_io::Project::decode(&pb_bytes[..]).unwrap();

        // protobuf -> datamodel -> sdai -> string
        let dm_decoded: datamodel::Project = project_serde::deserialize(pb_decoded);
        let sdai_decoded: SdaiModel = dm_decoded.into();
        let json_str = serde_json::to_string(&sdai_decoded).unwrap();

        (pb_bytes, json_str)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Test that protobuf roundtrips are idempotent after the first conversion.
        #[test]
        fn protobuf_json_roundtrip_idempotent(model in sdai_model_strategy()) {
            // First roundtrip: sdai -> pb -> json
            let (pb_bytes1, json_str1) = roundtrip_sdai_pb_json(&model);

            // Parse the JSON string back to an SdaiModel
            let sdai_parsed1: SdaiModel = serde_json::from_str(&json_str1).unwrap();

            // Second roundtrip: sdai -> pb -> json
            let (pb_bytes2, json_str2) = roundtrip_sdai_pb_json(&sdai_parsed1);

            // After first roundtrip, results should be stable
            prop_assert_eq!(
                pb_bytes1, pb_bytes2,
                "Protobuf bytes should be identical after roundtrip"
            );
            prop_assert_eq!(
                json_str1, json_str2,
                "JSON strings should be identical after roundtrip"
            );
        }
    }

    #[test]
    fn test_sdai_model_to_protobuf_roundtrip() {
        let sdai_model = SdaiModel {
            variables: vec![
                Variable::Stock(StockFields {
                    name: "inventory".to_string(),
                    equation: Some("100".to_string()),
                    documentation: Some("Current inventory level".to_string()),
                    units: Some("widgets".to_string()),
                    inflows: Some(vec!["production".to_string()]),
                    outflows: Some(vec!["sales".to_string()]),
                    graphical_function: None,
                }),
                Variable::Flow(FlowFields {
                    name: "production".to_string(),
                    equation: Some("10".to_string()),
                    documentation: None,
                    units: Some("widgets/month".to_string()),
                    graphical_function: None,
                }),
                Variable::Flow(FlowFields {
                    name: "sales".to_string(),
                    equation: Some("8".to_string()),
                    documentation: None,
                    units: Some("widgets/month".to_string()),
                    graphical_function: None,
                }),
                Variable::Variable(AuxiliaryFields {
                    name: "target_inventory".to_string(),
                    equation: Some("200".to_string()),
                    documentation: None,
                    units: None,
                    graphical_function: None,
                }),
            ],
            relationships: Some(vec![Relationship {
                reasoning: Some("Higher production leads to more inventory".to_string()),
                from: "production".to_string(),
                to: "inventory".to_string(),
                polarity: Polarity::Positive,
                polarity_reasoning: None,
            }]),
            specs: Some(SimSpecs {
                start_time: 0.0,
                stop_time: 100.0,
                dt: Some(1.0),
                time_units: Some("months".to_string()),
                save_step: None,
                method: None,
            }),
            views: None,
        };

        // First roundtrip
        let (pb_bytes1, json_str1) = roundtrip_sdai_pb_json(&sdai_model);

        // Parse and second roundtrip
        let sdai_parsed: SdaiModel = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_sdai_pb_json(&sdai_parsed);

        // Verify idempotence
        assert_eq!(pb_bytes1, pb_bytes2, "Protobuf bytes should be identical");
        assert_eq!(json_str1, json_str2, "JSON strings should be identical");

        // Verify the parsed model has the expected structure
        assert_eq!(sdai_parsed.variables.len(), 4);
        assert!(sdai_parsed.specs.is_some());
    }

    #[test]
    fn test_sdai_with_graphical_function_roundtrip() {
        let sdai_model = SdaiModel {
            variables: vec![Variable::Flow(FlowFields {
                name: "effect".to_string(),
                equation: Some("effect_lookup(input)".to_string()),
                documentation: None,
                units: None,
                graphical_function: Some(GraphicalFunction {
                    points: vec![
                        Point { x: 0.0, y: 0.0 },
                        Point { x: 0.5, y: 0.25 },
                        Point { x: 1.0, y: 1.0 },
                    ],
                }),
            })],
            relationships: None,
            specs: Some(SimSpecs {
                start_time: 0.0,
                stop_time: 10.0,
                dt: Some(0.25),
                time_units: None,
                save_step: None,
                method: None,
            }),
            views: None,
        };

        let (pb_bytes1, json_str1) = roundtrip_sdai_pb_json(&sdai_model);
        let sdai_parsed: SdaiModel = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_sdai_pb_json(&sdai_parsed);

        assert_eq!(pb_bytes1, pb_bytes2);
        assert_eq!(json_str1, json_str2);

        // Verify graphical function is preserved
        if let Variable::Flow(flow) = &sdai_parsed.variables[0] {
            assert!(flow.graphical_function.is_some());
            let gf = flow.graphical_function.as_ref().unwrap();
            assert_eq!(gf.points.len(), 3);
        } else {
            panic!("Expected flow variable");
        }
    }
}
