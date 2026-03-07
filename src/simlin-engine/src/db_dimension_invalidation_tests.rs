// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for dimension-granularity invalidation (AC8).
//!
//! Verifies that the per-variable dimension filtering in
//! `parse_source_variable_impl` correctly narrows salsa's invalidation
//! scope: scalar variables never depend on any dimension, arrayed
//! variables only depend on their own dimensions (plus transitive
//! maps_to targets), and unrelated dimension changes do not trigger
//! re-compilation.

use super::*;
use crate::datamodel;

/// Parse with an empty module-ident context (test convenience).
fn parse_var_no_module_ctx(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> &ParsedVariableResult {
    parse_source_variable_with_module_context(db, var, project, ModuleIdentContext::new(db, vec![]))
}

/// AC8.1: A scalar variable should be immune to dimension changes.
/// Changing dimension A must not invalidate the parse cache for a scalar.
#[test]
fn test_dimension_invalidation_scalar_immune() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "dim_inv".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["a1".to_string(), "a2".to_string()],
            ),
            datamodel::Dimension::named(
                "DimB".to_string(),
                vec!["b1".to_string(), "b2".to_string()],
            ),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["DimA".to_string()],
                        "x + 1".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();

    let x_src = sync1.models["main"].variables["x"].source;
    let x_ptr_before =
        parse_var_no_module_ctx(&db, x_src, sync1.project) as *const ParsedVariableResult;

    // Modify DimA: add an element
    let mut project2 = project.clone();
    project2.dimensions[0] = datamodel::Dimension::named(
        "DimA".to_string(),
        vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();

    let x_src2 = sync2.models["main"].variables["x"].source;
    let x_ptr_after =
        parse_var_no_module_ctx(&db, x_src2, sync2.project) as *const ParsedVariableResult;

    assert_eq!(
        x_ptr_before, x_ptr_after,
        "AC8.1: scalar variable x should be cached (pointer-equal) after DimA change"
    );
}

/// AC8.2: An arrayed variable referencing only DimB should produce a
/// value-equal parse result when DimA changes.
///
/// The parse function re-executes because `project.dimensions(db)` changed
/// (needed for `expand_maps_to_chains`), but after filtering to only
/// DimB-relevant dimensions the same dims are passed to the parser,
/// producing a structurally equal result. Salsa's early-cutoff then
/// prevents further downstream invalidation.
#[test]
fn test_dimension_invalidation_different_dim_immune() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "dim_inv".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["a1".to_string(), "a2".to_string()],
            ),
            datamodel::Dimension::named(
                "DimB".to_string(),
                vec!["b1".to_string(), "b2".to_string()],
            ),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::ApplyToAll(
                    vec!["DimB".to_string()],
                    "5".to_string(),
                ),
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
        }],
        source: None,
        ai_information: None,
    };

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();

    let y_src = sync1.models["main"].variables["y"].source;
    let y_result_before = parse_var_no_module_ctx(&db, y_src, sync1.project).clone();

    // Modify DimA only: add an element
    let mut project2 = project.clone();
    project2.dimensions[0] = datamodel::Dimension::named(
        "DimA".to_string(),
        vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();

    let y_src2 = sync2.models["main"].variables["y"].source;
    let y_result_after = parse_var_no_module_ctx(&db, y_src2, sync2.project).clone();

    assert_eq!(
        y_result_before, y_result_after,
        "AC8.2: variable y[DimB] parse result should be value-equal after DimA change"
    );

    // Also verify that the compile_var_fragment output is cached via
    // salsa early-cutoff: since the parse result is equal, downstream
    // compilation should produce value-equal fragments.
    let model1 = sync1.models["main"].source;
    let model2 = sync2.models["main"].source;

    let frag1 = compile_var_fragment(&db, y_src, model1, sync1.project, true, vec![])
        .as_ref()
        .unwrap()
        .fragment
        .clone();
    let frag2 = compile_var_fragment(&db, y_src2, model2, sync2.project, true, vec![])
        .as_ref()
        .unwrap()
        .fragment
        .clone();

    assert_eq!(
        frag1, frag2,
        "AC8.2: y[DimB] fragment should be value-equal after DimA change (early cutoff)"
    );
}

/// AC8.3: An arrayed variable referencing DimA should be re-parsed when
/// DimA changes.
#[test]
fn test_dimension_invalidation_same_dim_reparsed() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "dim_inv".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![
            datamodel::Dimension::named(
                "DimA".to_string(),
                vec!["a1".to_string(), "a2".to_string()],
            ),
            datamodel::Dimension::named(
                "DimB".to_string(),
                vec!["b1".to_string(), "b2".to_string()],
            ),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::ApplyToAll(
                    vec!["DimA".to_string()],
                    "5".to_string(),
                ),
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
        }],
        source: None,
        ai_information: None,
    };

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();

    let y_src = sync1.models["main"].variables["y"].source;
    let y_ptr_before =
        parse_var_no_module_ctx(&db, y_src, sync1.project) as *const ParsedVariableResult;

    // Modify DimA: add an element -- y references DimA so it must be re-parsed
    let mut project2 = project.clone();
    project2.dimensions[0] = datamodel::Dimension::named(
        "DimA".to_string(),
        vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();

    let y_src2 = sync2.models["main"].variables["y"].source;
    let y_ptr_after =
        parse_var_no_module_ctx(&db, y_src2, sync2.project) as *const ParsedVariableResult;

    assert_ne!(
        y_ptr_before, y_ptr_after,
        "AC8.3: variable y[DimA] should be re-parsed (different pointer) after DimA change"
    );
}

/// AC8.3 (maps_to): When DimA maps_to DimB, changing DimB should trigger
/// a re-parse of a variable that references DimA, because the expanded
/// relevant set includes both A and B.
#[test]
fn test_dimension_invalidation_maps_to_chain() {
    let mut db = SimlinDb::default();

    let mut dim_a =
        datamodel::Dimension::named("DimA".to_string(), vec!["a1".to_string(), "a2".to_string()]);
    dim_a.set_maps_to("DimB".to_string());

    let project = datamodel::Project {
        name: "dim_inv".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![
            dim_a,
            datamodel::Dimension::named(
                "DimB".to_string(),
                vec!["b1".to_string(), "b2".to_string()],
            ),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::ApplyToAll(
                    vec!["DimA".to_string()],
                    "5".to_string(),
                ),
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
        }],
        source: None,
        ai_information: None,
    };

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();

    let y_src = sync1.models["main"].variables["y"].source;
    let y_ptr_before =
        parse_var_no_module_ctx(&db, y_src, sync1.project) as *const ParsedVariableResult;

    // Modify DimB (which DimA maps_to): change its elements.
    // y references DimA, and DimA maps_to DimB, so y must be re-parsed.
    let mut project2 = project.clone();
    let mut dim_a2 =
        datamodel::Dimension::named("DimA".to_string(), vec!["a1".to_string(), "a2".to_string()]);
    dim_a2.set_maps_to("DimB".to_string());
    project2.dimensions[0] = dim_a2;
    project2.dimensions[1] = datamodel::Dimension::named(
        "DimB".to_string(),
        vec!["b1".to_string(), "b2".to_string(), "b3".to_string()],
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();

    let y_src2 = sync2.models["main"].variables["y"].source;
    let y_ptr_after =
        parse_var_no_module_ctx(&db, y_src2, sync2.project) as *const ParsedVariableResult;

    assert_ne!(
        y_ptr_before, y_ptr_after,
        "AC8.3: variable y[DimA] should be re-parsed when DimB changes (DimA maps_to DimB)"
    );
}
