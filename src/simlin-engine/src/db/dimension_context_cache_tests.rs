// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the project-global dimension-context salsa queries.
//!
//! `project_dimensions_context` and `project_converted_dimensions` compute the
//! project's `DimensionsContext` and converted `Vec<Dimension>` once per
//! project (keyed on the `SourceProject` dimensions input), so the per-variable
//! compile sites can read the cached value instead of rebuilding it on every
//! variable. What needs a test is the INVALIDATION: caching them off
//! `project_datamodel_dims` must keep the same granularity the inline rebuild
//! had, so a dimension edit is still seen (mirroring
//! `db/dimension_invalidation_tests.rs`). Equality against a fresh rebuild is
//! not testable here -- each query body IS that rebuild.

use super::*;
use crate::datamodel;

/// Build a two-dimension project with a single scalar variable.
fn two_dim_project() -> datamodel::Project {
    datamodel::Project {
        name: "dim_ctx_cache".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["a1".to_string(), "a2".to_string()],
            ),
            datamodel::Dimension::named(
                "DimB".to_string(),
                vec!["b1".to_string(), "b2".to_string(), "b3".to_string()],
            ),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Changing a dimension recomputes the context queries to the new value
/// (the same input-dependency the per-variable sites already took by reading
/// `project_datamodel_dims`).
#[test]
fn test_dimension_context_recomputes_on_dimension_change() {
    let mut db = SimlinDb::default();
    let project = two_dim_project();
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();

    let dim_a = crate::common::CanonicalDimensionName::from_raw("DimA");
    let len_before = project_dimensions_context(&db, sync1.project)
        .get(&dim_a)
        .map(|d| d.len());
    assert_eq!(len_before, Some(2), "DimA starts with 2 elements");
    assert_eq!(
        project_converted_dimensions(&db, sync1.project).len(),
        2,
        "two dimensions before the change"
    );

    // Add an element to DimA.
    let mut project2 = project.clone();
    project2.dimensions[0] = datamodel::Dimension::named(
        "DimA".to_string(),
        vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
    );
    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();

    let len_after = project_dimensions_context(&db, sync2.project)
        .get(&dim_a)
        .map(|d| d.len());
    assert_eq!(
        len_after,
        Some(3),
        "project_dimensions_context must recompute after DimA grows"
    );

    // Cross-check against a freshly-built context to pin behavior-preservation.
    let fresh = crate::dimensions::DimensionsContext::from(
        project_datamodel_dims(&db, sync2.project).as_slice(),
    );
    assert_eq!(*project_dimensions_context(&db, sync2.project), fresh);
}
