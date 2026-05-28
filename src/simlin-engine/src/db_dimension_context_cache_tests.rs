// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the project-global dimension-context salsa queries.
//!
//! `project_dimensions_context` and `project_converted_dimensions` compute the
//! project's `DimensionsContext` and converted `Vec<Dimension>` once per
//! project (keyed on the `SourceProject` dimensions input), so the per-variable
//! compile sites can read the cached value instead of rebuilding it on every
//! variable. These tests verify:
//!   - the cached values equal the freshly-built ones (oracle), and
//!   - the queries are memoized across calls / recompute when a dimension
//!     changes (mirroring `db_dimension_invalidation_tests.rs`).

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

/// The cached `project_dimensions_context` must equal a context built fresh
/// from `project_datamodel_dims` (the exact computation the per-variable sites
/// used to do inline).
#[test]
fn test_cached_dimensions_context_equals_fresh() {
    let mut db = SimlinDb::default();
    let project = two_dim_project();
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();

    let cached = project_dimensions_context(&db, sync.project);
    let fresh = crate::dimensions::DimensionsContext::from(
        project_datamodel_dims(&db, sync.project).as_slice(),
    );

    assert_eq!(
        *cached, fresh,
        "cached project_dimensions_context must equal the freshly-built context"
    );
}

/// The cached `project_converted_dimensions` must equal the per-`Dimension`
/// conversion the sites used to do inline.
#[test]
fn test_cached_converted_dimensions_equals_fresh() {
    let mut db = SimlinDb::default();
    let project = two_dim_project();
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();

    let cached = project_converted_dimensions(&db, sync.project);
    let fresh: Vec<crate::dimensions::Dimension> = project_datamodel_dims(&db, sync.project)
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    assert_eq!(
        *cached, fresh,
        "cached project_converted_dimensions must equal the freshly-built vec"
    );
}

/// Both queries are `returns(ref)` and keyed only on the project's dimensions,
/// so two calls without an intervening dimension change return the SAME cached
/// allocation (pointer-equal) -- proving memoization (no per-call rebuild).
#[test]
fn test_dimension_context_queries_are_memoized() {
    let mut db = SimlinDb::default();
    let project = two_dim_project();
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();

    let ctx_ptr_a = project_dimensions_context(&db, sync.project)
        as *const crate::dimensions::DimensionsContext;
    let ctx_ptr_b = project_dimensions_context(&db, sync.project)
        as *const crate::dimensions::DimensionsContext;
    assert_eq!(
        ctx_ptr_a, ctx_ptr_b,
        "project_dimensions_context must be memoized (cache hit, pointer-equal)"
    );

    let conv_ptr_a =
        project_converted_dimensions(&db, sync.project) as *const Vec<crate::dimensions::Dimension>;
    let conv_ptr_b =
        project_converted_dimensions(&db, sync.project) as *const Vec<crate::dimensions::Dimension>;
    assert_eq!(
        conv_ptr_a, conv_ptr_b,
        "project_converted_dimensions must be memoized (cache hit, pointer-equal)"
    );
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
