// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for input-granularity invalidation (AC8).
//!
//! Verifies that the per-variable dimension filtering in
//! `parse_source_variable_impl` correctly narrows salsa's invalidation
//! scope: scalar variables never depend on any dimension, arrayed
//! variables only depend on their own dimensions (plus transitive
//! maps_to targets), and unrelated dimension changes do not trigger
//! re-compilation.
//!
//! The same question for the project's UNITS input is at the bottom of the
//! file: a reader of `project_units_context` must not be invalidated by a
//! change that moves only the unit-definition ERROR list.

use super::*;
use crate::datamodel;

/// A qualified snapshot selector depends on the one qualified name it uses,
/// not the project's complete dimension context. The selector dimension is
/// deliberately unrelated to `vals`' declared axis: qualification constifies
/// to a 1-based position, and subscript normalization applies that position to
/// the referenced axis.
#[test]
fn qualified_snapshot_position_has_per_name_invalidation() {
    use crate::db::exec_probe::ProbedDb;
    use crate::test_common::TestProject;

    let base = TestProject::new("qualified_snapshot_invalidation")
        .named_dimension("Data", &["d1", "d2", "d3"])
        .named_dimension("Selector", &["s1", "s2", "s3"])
        .named_dimension("Unrelated", &["u1", "u2"])
        .array_with_ranges("vals[Data]", vec![("d1", "10"), ("d2", "20"), ("d3", "30")])
        .scalar_aux("probe", "PREVIOUS(vals[Selector.s2], 0)")
        .build_datamodel();

    let parse_probe = |db: &ProbedDb, sync: &SyncResult| {
        parse_source_variable(
            db.db(),
            sync.models["main"].variables["probe"].source,
            sync.project,
        );
    };

    let mut db = ProbedDb::new();
    let state1 = sync_from_datamodel_incremental(db.db_mut(), &base, None);
    parse_probe(&db, &state1.to_sync_result());

    let mut unrelated_edit = base.clone();
    unrelated_edit.dimensions[2] =
        datamodel::Dimension::named("Unrelated".to_string(), vec!["u1".into(), "u3".into()]);
    db.reset();
    let state2 = sync_from_datamodel_incremental(db.db_mut(), &unrelated_edit, Some(&state1));
    parse_probe(&db, &state2.to_sync_result());
    assert_eq!(
        db.counts().get("parse_source_variable"),
        None,
        "an unrelated dimension edit must backdate the qualified-position projection before parse"
    );
    assert_eq!(
        db.counts()
            .get("project_qualified_snapshot_position")
            .map(|(runs, _)| *runs),
        Some(1),
        "the per-name projection must re-check the vector input before salsa can backdate it"
    );

    let mut position_edit = unrelated_edit.clone();
    position_edit.dimensions[1] = datamodel::Dimension::named(
        "Selector".to_string(),
        vec!["s2".into(), "s1".into(), "s3".into()],
    );
    db.reset();
    let state3 = sync_from_datamodel_incremental(db.db_mut(), &position_edit, Some(&state2));
    parse_probe(&db, &state3.to_sync_result());
    assert_eq!(
        db.counts()
            .get("parse_source_variable")
            .map(|(runs, _)| *runs),
        Some(1),
        "moving the selected element must invalidate the source parse"
    );
    assert_eq!(
        db.counts()
            .get("project_qualified_snapshot_position")
            .map(|(runs, _)| *runs),
        Some(1),
        "moving the selected element must change the per-name projection"
    );

    let mut name_edit = position_edit.clone();
    name_edit.dimensions[1] = datamodel::Dimension::named(
        "Selector".to_string(),
        vec!["renamed".into(), "s1".into(), "s3".into()],
    );
    db.reset();
    let state4 = sync_from_datamodel_incremental(db.db_mut(), &name_edit, Some(&state3));
    parse_probe(&db, &state4.to_sync_result());
    assert_eq!(
        db.counts()
            .get("parse_source_variable")
            .map(|(runs, _)| *runs),
        Some(1),
        "removing the selected qualified name must invalidate the source parse"
    );
    assert_eq!(
        db.counts()
            .get("project_qualified_snapshot_position")
            .map(|(runs, _)| *runs),
        Some(1),
        "removing the selected element must change the per-name projection to None"
    );
}

/// Parse one source variable (test convenience).
fn parse_var_no_module_ctx(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> &ParsedVariableResult {
    parse_source_variable(db, var, project)
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
            macro_spec: None,
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
            macro_spec: None,
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

    let frag1 = compile_var_fragment(
        &db,
        y_src,
        model1,
        sync1.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();
    let frag2 = compile_var_fragment(
        &db,
        y_src2,
        model2,
        sync2.project,
        ModuleInputSet::empty(&db),
    )
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
            macro_spec: None,
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
            macro_spec: None,
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

// ── expand_maps_to_chains: canonical-vs-display reachability (GH #580 Bug A) ──

mod expand_maps_to_chains_tests {
    use super::super::expand_maps_to_chains;
    use crate::datamodel::{Dimension, DimensionMapping};
    use std::collections::BTreeSet;

    fn named(name: &str, elements: &[&str]) -> Dimension {
        Dimension::named(
            name.to_string(),
            elements.iter().map(|s| s.to_string()).collect(),
        )
    }

    /// Reverse reachability: a variable subscripted by the *target* dimension
    /// must pull the mapping *source* into the set so cross-dimension subscript
    /// substitution can see it. `Dimension.name` keeps the as-written display
    /// casing while the importers store `mappings[].target` canonical
    /// (lowercase), so the reverse pass must compare on the canonical form --
    /// before the GH #580 Bug A fix the raw `==` here dropped the source
    /// dimension, leaving the bare full-dimension subscript that lowered to
    /// `DimensionInScalarContext`.
    #[test]
    fn reverse_pulls_in_group_mapped_source_despite_case_skew() {
        // `Small` (display) maps to `big` (canonical target) as a group mapping.
        let mut small = named("Small", &["s1", "s2"]);
        small.mappings = vec![DimensionMapping {
            target: "big".to_string(), // canonical, as the MDL/XMILE importers store it
            element_map: vec![
                ("s1".to_string(), "e1".to_string()),
                ("s1".to_string(), "e2".to_string()),
                ("s2".to_string(), "e3".to_string()),
                ("s2".to_string(), "e4".to_string()),
            ],
        }];
        let big = named("Big", &["e1", "e2", "e3", "e4"]);
        let all = [big, small];

        // A variable declared over `Big` (display casing in its ApplyToAll dims).
        let relevant: BTreeSet<String> = ["Big".to_string()].into_iter().collect();
        let expanded = expand_maps_to_chains(&relevant, &all);

        assert!(
            expanded.contains("Small"),
            "reverse mapping must pull display-named `Small` into the set even \
             though its mapping target `big` is canonical; got {expanded:?}"
        );
        assert!(expanded.contains("Big"));
    }

    /// Forward reachability: a variable subscripted by the *source* dimension
    /// must pull the mapping *target* in, resolved back to its display name so
    /// the caller's `expanded.contains(&d.name)` filter (display-keyed) matches.
    #[test]
    fn forward_pulls_in_target_resolved_to_display_name() {
        let mut small = named("Small", &["s1", "s2"]);
        small.mappings = vec![DimensionMapping {
            target: "big".to_string(),
            element_map: vec![],
        }];
        let big = named("Big", &["e1", "e2"]);
        let all = [big, small];

        let relevant: BTreeSet<String> = ["Small".to_string()].into_iter().collect();
        let expanded = expand_maps_to_chains(&relevant, &all);

        assert!(
            expanded.contains("Big"),
            "forward mapping must pull the target in under its DISPLAY name `Big` \
             (not the canonical `big`) so the caller's display-keyed filter \
             matches; got {expanded:?}"
        );
    }

    /// An unrelated dimension (no mapping in either direction) is excluded.
    #[test]
    fn unrelated_dimension_is_not_pulled_in() {
        let small = named("Small", &["s1", "s2"]);
        let big = named("Big", &["e1", "e2"]);
        let unrelated = named("Unrelated", &["u1"]);
        let all = [big, small, unrelated];

        let relevant: BTreeSet<String> = ["Big".to_string()].into_iter().collect();
        let expanded = expand_maps_to_chains(&relevant, &all);

        assert!(
            !expanded.contains("Small"),
            "no mapping relates Small to Big"
        );
        assert!(!expanded.contains("Unrelated"));
        assert_eq!(expanded, relevant);
    }

    /// The legacy `maps_to` field path is canonicalized on both sides too.
    #[test]
    fn reverse_pulls_in_maps_to_source_despite_case_skew() {
        let mut child = named("Child", &["c1", "c2"]);
        child.set_maps_to("parent".to_string()); // canonical
        let parent = named("Parent", &["p1", "p2"]);
        let all = [parent, child];

        let relevant: BTreeSet<String> = ["Parent".to_string()].into_iter().collect();
        let expanded = expand_maps_to_chains(&relevant, &all);

        assert!(
            expanded.contains("Child"),
            "reverse `maps_to` must also compare canonically; got {expanded:?}"
        );
    }
}

// ---- units-granularity invalidation ----

/// A change that alters ONLY the unit-definition errors must not invalidate
/// readers of `project_units_context`.
///
/// The two halves of `UnitsContextResult` move independently: a unit whose
/// equation fails to parse is left out of the context entirely, so replacing one
/// malformed equation with a different malformed one changes
/// `definition_errors` while `ctx` stays byte-identical. That is not a contrived
/// case -- it is every intermediate state of typing a unit equation in the
/// editor.
///
/// `project_units_context` is therefore a salsa PROJECTION over the result, not
/// a plain accessor: it re-executes but backdates on the equal `Context`, so its
/// readers are left alone. With a plain accessor every reader takes a dependency
/// on the whole result and rebuilds on each keystroke -- measured here at the
/// production per-variable `lowered_source_variable` projection.
#[test]
fn unit_definition_error_only_change_does_not_invalidate_context_readers() {
    use crate::db::exec_probe::ProbedDb;
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

    let project_with_malformed_unit = |eqn: &str| {
        let mut dm = x_project(
            sim_specs_with_units("month"),
            &[x_model("main", vec![x_aux("x", "1", None)])],
        );
        dm.units.push(datamodel::Unit {
            name: "bad".to_string(),
            equation: Some(eqn.to_string()),
            disabled: false,
            aliases: vec![],
        });
        dm
    };

    let mut db = ProbedDb::new();
    let first = project_with_malformed_unit("widget/");
    let state1 = sync_from_datamodel_incremental(db.db_mut(), &first, None);
    let sync1 = state1.to_sync_result();
    let x1 = sync1.models["main"].variables["x"].source;
    let _ = lowered_source_variable(db.db(), x1, sync1.models["main"].source, sync1.project);

    // Control: re-syncing the identical project rebuilds nothing, so a rebuild
    // below is attributable to the edit and not to the re-sync itself.
    db.reset();
    let state2 = sync_from_datamodel_incremental(db.db_mut(), &first, Some(&state1));
    let sync2 = state2.to_sync_result();
    let _ = lowered_source_variable(
        db.db(),
        sync2.models["main"].variables["x"].source,
        sync2.models["main"].source,
        sync2.project,
    );
    assert_eq!(
        db.counts().get("lowered_source_variable"),
        None,
        "re-syncing an unchanged project must not rebuild anything"
    );

    // Snapshot BEFORE the edit: incremental sync reuses the same `SourceProject`
    // handle and mutates its fields, so `sync2.project` and `sync3.project` are
    // the same salsa input -- reading through it after the edit yields the NEW
    // value for both.
    let ctx_before = project_units_context(db.db(), sync2.project).clone();
    let errors_before = project_units_context_result(db.db(), sync2.project)
        .definition_errors
        .clone();

    // A DIFFERENT malformed equation for the same unit: both are rejected.
    let second = project_with_malformed_unit("widget * ");
    db.reset();
    let state3 = sync_from_datamodel_incremental(db.db_mut(), &second, Some(&state2));
    let sync3 = state3.to_sync_result();

    assert_eq!(
        *project_units_context(db.db(), sync3.project),
        ctx_before,
        "the fixture must leave the units context unchanged, or it proves nothing"
    );
    let errors_after = &project_units_context_result(db.db(), sync3.project).definition_errors;
    assert_ne!(
        errors_after, &errors_before,
        "the fixture must change the definition errors, or it proves nothing"
    );

    let _ = lowered_source_variable(
        db.db(),
        sync3.models["main"].variables["x"].source,
        sync3.models["main"].source,
        sync3.project,
    );
    assert_eq!(
        db.counts().get("lowered_source_variable"),
        None,
        "a change to the unit-definition errors alone must not rebuild a reader \
         of the units context; errors are now {errors_after:?}"
    );
}
