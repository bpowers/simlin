// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;

#[test]
fn source_dimension_preserves_element_level_mappings() {
    let dim = datamodel::Dimension {
        name: "dim_a".to_string(),
        elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
        mappings: vec![datamodel::DimensionMapping {
            target: "dim_b".to_string(),
            element_map: vec![
                ("a1".to_string(), "b2".to_string()),
                ("a2".to_string(), "b1".to_string()),
            ],
        }],
        parent: None,
    };
    let source: SourceDimension = SourceDimension::from(&dim);
    let roundtripped = source_dims_to_datamodel(&[source]);
    assert_eq!(roundtripped.len(), 1);
    assert_eq!(roundtripped[0].mappings.len(), 1);
    assert_eq!(roundtripped[0].mappings[0].target, "dim_b");
    assert_eq!(roundtripped[0].mappings[0].element_map.len(), 2);
}

#[test]
fn source_dimension_preserves_multi_target_positional_mappings() {
    let dim = datamodel::Dimension {
        name: "dim_a".to_string(),
        elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
        mappings: vec![
            datamodel::DimensionMapping {
                target: "dim_b".to_string(),
                element_map: vec![],
            },
            datamodel::DimensionMapping {
                target: "dim_c".to_string(),
                element_map: vec![],
            },
        ],
        parent: None,
    };
    let source: SourceDimension = SourceDimension::from(&dim);
    let roundtripped = source_dims_to_datamodel(&[source]);
    assert_eq!(roundtripped.len(), 1);
    assert_eq!(
        roundtripped[0].mappings.len(),
        2,
        "both positional mappings must survive DB round-trip"
    );
}

#[test]
fn source_dimension_preserves_parent() {
    let dim = datamodel::Dimension {
        name: "child_dim".to_string(),
        elements: datamodel::DimensionElements::Indexed(3),
        mappings: vec![],
        parent: Some("parent_dim".to_string()),
    };
    let source: SourceDimension = SourceDimension::from(&dim);
    assert_eq!(source.parent.as_deref(), Some("parent_dim"));
    let roundtripped = source_dims_to_datamodel(&[source]);
    assert_eq!(roundtripped.len(), 1);
    assert_eq!(
        roundtripped[0].parent.as_deref(),
        Some("parent_dim"),
        "parent must survive DB round-trip"
    );
}

#[test]
fn source_equation_preserves_default_equation() {
    let eq = datamodel::Equation::Arrayed(
        vec!["DimA".to_string()],
        vec![("A1".to_string(), "5".to_string(), None, None)],
        Some("default_val".to_string()),
        true,
    );
    let source = SourceEquation::from(&eq);
    let roundtripped = source_equation_to_datamodel(&source);
    match &roundtripped {
        datamodel::Equation::Arrayed(_, _, default_eq, _) => {
            assert_eq!(
                default_eq.as_deref(),
                Some("default_val"),
                "default_equation must survive DB round-trip"
            );
        }
        _ => panic!("Expected Arrayed equation"),
    }
}
