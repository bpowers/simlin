// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;

#[test]
fn test_model_dependency_graph_prunes_lagged_deps_for_implicit_helpers() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("TIME".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "z".to_string(),
                    equation: datamodel::Equation::Scalar("PREVIOUS(PREVIOUS(x))".to_string()),
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

    let result = sync_from_datamodel(&db, &project);
    let source_model = result.models["main"].source;
    let graph = model_dependency_graph(
        &db,
        source_model,
        result.project,
        ModuleInputSet::empty(&db),
    );
    let helper = graph
        .dt_dependencies
        .iter()
        .find(|(name, _)| name.as_str().contains("arg0"))
        .expect("nested PREVIOUS should create an implicit arg helper");

    assert!(
        !helper.1.contains("x"),
        "dependency graph should prune lagged PREVIOUS(x) edge from helper dt deps"
    );
    assert!(
        !graph
            .initial_dependencies
            .get(helper.0)
            .is_some_and(|deps| deps.contains("x")),
        "dependency graph should prune lagged PREVIOUS(x) edge from helper initial deps"
    );
}

#[test]
fn test_nested_previous_does_not_create_false_cycle_via_helper_deps() {
    use crate::test_common::TestProject;

    // z(t) = x(t-2) is lagged and should not form a same-step cycle with x.
    let tp = TestProject::new("nested_previous_no_false_cycle")
        .with_sim_time(0.0, 4.0, 1.0)
        .aux("x", "z + 1", None)
        .aux("z", "PREVIOUS(PREVIOUS(x))", None);

    tp.assert_compiles_incremental();

    let vm = tp.run_vm().expect("VM should run");
    let x_vals = vm.get("x").expect("x not in VM results");
    let z_vals = vm.get("z").expect("z not in VM results");

    assert!(
        (x_vals[0] - 1.0).abs() < 1e-10,
        "x at t=0 should be 1 (z starts at 0), got {}",
        x_vals[0]
    );
    assert!(
        (z_vals[0] - 0.0).abs() < 1e-10,
        "z at t=0 should be 0 due to PREVIOUS defaults, got {}",
        z_vals[0]
    );
}

/// PREVIOUS of an array element selected by a *qualified* `Dim.element`
/// subscript compiles to a direct LoadPrev at that element's slot -- no
/// implicit helper aux is synthesized.
///
/// The qualified form is unambiguous: a dimension name can never collide with
/// a variable name (XMILE 3.7.1), so `DimA.a2` inside a subscript is always
/// the element constant, never a variable reference. Bare element names stay
/// on the helper path: XMILE allows element names to shadow variable names,
/// so `arr[a2]` could in principle be a dynamic index through a variable
/// named `a2`.
///
/// The "no helper" assertion runs against `parse_var` with the project
/// dimensions in scope -- the same way the LTM equation parse path
/// (`parse_ltm_equation`) and the per-element A2A path call it. The salsa
/// parse path for *user* scalar variables deliberately passes no dimensions
/// (dimension-granularity invalidation: a dimension edit must not re-parse
/// every scalar variable), so user scalar equations keep the helper-aux
/// rewrite there; values are identical either way, pinned by the VM half of
/// this test.
#[test]
fn test_previous_qualified_element_subscript_no_helper() {
    use crate::test_common::TestProject;

    // Parse-level contract: with dimensions in scope, no helper is created.
    let dims = vec![datamodel::Dimension::named(
        "DimA".to_string(),
        vec!["a1".to_string(), "a2".to_string()],
    )];
    let aux = datamodel::Variable::Aux(datamodel::Aux {
        ident: "lagged".to_string(),
        equation: datamodel::Equation::Scalar("PREVIOUS(base_val[DimA.a2], 0)".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let parsed: crate::variable::Variable<datamodel::ModuleReference, crate::ast::Expr0> =
        crate::variable::parse_var(&dims, &aux, &mut implicit_vars, &units_ctx, |mi| {
            Ok(Some(mi.clone()))
        });
    assert!(
        parsed.equation_errors().is_none(),
        "equation should parse cleanly: {:?}",
        parsed.equation_errors()
    );
    assert!(
        implicit_vars.is_empty(),
        "qualified-element PREVIOUS must not synthesize helper vars; got: {:?}",
        implicit_vars
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect::<Vec<_>>()
    );

    // End-to-end values through the production (salsa) compile path.
    let tp = TestProject::new("prev_qualified_elem")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges("base_val[DimA]", vec![("a1", "10"), ("a2", "20")])
        .aux("lagged", "PREVIOUS(base_val[DimA.a2], 0)", None);

    tp.assert_compiles_incremental();

    // Values: the explicit fallback (0) at t=0, then base_val[a2] = 20.
    let vm = tp.run_vm().expect("VM should run");
    let lagged = vm.get("lagged").expect("lagged not in VM results");
    assert!(
        (lagged[0] - 0.0).abs() < 1e-10,
        "lagged at t=0 should be the fallback 0, got {}",
        lagged[0]
    );
    for (step, val) in lagged.iter().enumerate().skip(1) {
        assert!(
            (val - 20.0).abs() < 1e-10,
            "lagged at step {step} should be 20, got {val}"
        );
    }
}

/// PREVIOUS with a numeric-constant subscript index also compiles to a
/// direct LoadPrev (a number is never a variable reference).
#[test]
fn test_previous_numeric_subscript_no_helper() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("prev_numeric_elem")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges("base_val[DimA]", vec![("a1", "10"), ("a2", "20")])
        .aux("lagged", "PREVIOUS(base_val[2], 0)", None);

    tp.assert_compiles_incremental();

    let db = SimlinDb::default();
    let project = tp.build_datamodel();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let info = model_implicit_var_info(&db, source_model, sync.project);
    assert!(
        info.is_empty(),
        "numeric-index PREVIOUS must not synthesize helper vars; got: {:?}",
        info.keys().collect::<Vec<_>>()
    );

    let vm = tp.run_vm().expect("VM should run");
    let lagged = vm.get("lagged").expect("lagged not in VM results");
    assert!((lagged[0] - 0.0).abs() < 1e-10);
    for (step, val) in lagged.iter().enumerate().skip(1) {
        assert!(
            (val - 20.0).abs() < 1e-10,
            "lagged at step {step} should be 20, got {val}"
        );
    }
}

/// PREVIOUS with a *dynamic* subscript index (a variable) keeps the
/// helper-aux rewrite. The helper captures `arr[idx]` each step, so PREVIOUS
/// returns the value as of the previous step *with the previous step's
/// index* -- the correct lagged semantics (LoadPrev at a current-step index
/// would be wrong).
#[test]
fn test_previous_dynamic_subscript_keeps_helper() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("prev_dynamic_idx")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges("base_val[DimA]", vec![("a1", "10"), ("a2", "20")])
        .aux("idx", "1 + MIN(TIME, 1)", None)
        .aux("lagged", "PREVIOUS(base_val[idx], 0)", None);

    tp.assert_compiles_incremental();

    let db = SimlinDb::default();
    let project = tp.build_datamodel();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let info = model_implicit_var_info(&db, source_model, sync.project);
    assert!(
        !info.is_empty(),
        "dynamic-index PREVIOUS must keep the helper-aux rewrite"
    );

    // t=0: fallback 0. t=1: helper(t=0) = base_val[idx(0)] = base_val[1] = 10.
    // t=2: helper(t=1) = base_val[idx(1)] = base_val[2] = 20. t=3: 20.
    let vm = tp.run_vm().expect("VM should run");
    let lagged = vm.get("lagged").expect("lagged not in VM results");
    assert!((lagged[0] - 0.0).abs() < 1e-10, "t=0: {}", lagged[0]);
    assert!((lagged[1] - 10.0).abs() < 1e-10, "t=1: {}", lagged[1]);
    assert!((lagged[2] - 20.0).abs() < 1e-10, "t=2: {}", lagged[2]);
}

/// A2A PREVIOUS over the iterated dimension (`prev_val[DimA] =
/// PREVIOUS(base_val[DimA], 99)`) compiles each element to a direct
/// LoadPrev: the per-element dimension substitution turns `base_val[DimA]`
/// into the qualified `base_val[DimA·a1]` *before* the helper decision, and
/// the qualified form is statically resolvable.
///
/// Values for this model shape are pinned by
/// `test_arrayed_2arg_previous_per_element` (db/tests.rs); this test pins the
/// structural property that no helper vars exist.
#[test]
fn test_previous_a2a_iterated_dimension_no_helpers() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("prev_a2a_no_helpers")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges("base_val[DimA]", vec![("a1", "10"), ("a2", "20")])
        .array_aux("prev_val[DimA]", "PREVIOUS(base_val[DimA], 99)");

    tp.assert_compiles_incremental();

    let db = SimlinDb::default();
    let project = tp.build_datamodel();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let info = model_implicit_var_info(&db, source_model, sync.project);
    assert!(
        info.is_empty(),
        "A2A iterated-dimension PREVIOUS must not synthesize helper vars; got: {:?}",
        info.keys().collect::<Vec<_>>()
    );

    // Per-element values still correct through the direct path.
    let vm = tp.run_vm().expect("VM should run");
    let a1 = vm.get("prev_val[a1]").expect("prev_val[a1] in results");
    let a2 = vm.get("prev_val[a2]").expect("prev_val[a2] in results");
    assert!((a1[0] - 99.0).abs() < 1e-10, "a1 t=0: {}", a1[0]);
    assert!((a2[0] - 99.0).abs() < 1e-10, "a2 t=0: {}", a2[0]);
    for step in 1..a1.len() {
        assert!(
            (a1[step] - 10.0).abs() < 1e-10,
            "a1 step {step}: {}",
            a1[step]
        );
        assert!(
            (a2[step] - 20.0).abs() < 1e-10,
            "a2 step {step}: {}",
            a2[step]
        );
    }
}

/// INIT with a qualified-element subscript also compiles directly to
/// LoadInitial -- the same static-resolution rule PREVIOUS uses.
///
/// Like the PREVIOUS twin above, the "no helper" half of this test runs
/// `parse_var` with dimensions in scope (the LTM / per-element parse
/// configuration); the VM half pins values through the production path.
#[test]
fn test_init_qualified_element_subscript_no_helper() {
    use crate::test_common::TestProject;

    // Parse-level contract: with dimensions in scope, no helper is created.
    let dims = vec![datamodel::Dimension::named(
        "DimA".to_string(),
        vec!["a1".to_string(), "a2".to_string()],
    )];
    let aux = datamodel::Variable::Aux(datamodel::Aux {
        ident: "frozen".to_string(),
        equation: datamodel::Equation::Scalar("INIT(growing[DimA.a2])".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let parsed: crate::variable::Variable<datamodel::ModuleReference, crate::ast::Expr0> =
        crate::variable::parse_var(&dims, &aux, &mut implicit_vars, &units_ctx, |mi| {
            Ok(Some(mi.clone()))
        });
    assert!(
        parsed.equation_errors().is_none(),
        "equation should parse cleanly: {:?}",
        parsed.equation_errors()
    );
    assert!(
        implicit_vars.is_empty(),
        "qualified-element INIT must not synthesize helper vars; got: {:?}",
        implicit_vars
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect::<Vec<_>>()
    );

    // growing[DimA] grows each step; INIT freezes the t=0 value.
    let tp = TestProject::new("init_qualified_elem")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges(
            "growing[DimA]",
            vec![("a1", "10 + TIME"), ("a2", "20 + TIME")],
        )
        .aux("frozen", "INIT(growing[DimA.a2])", None);

    tp.assert_compiles_incremental();

    let vm = tp.run_vm().expect("VM should run");
    let frozen = vm.get("frozen").expect("frozen not in VM results");
    for (step, val) in frozen.iter().enumerate() {
        assert!(
            (val - 20.0).abs() < 1e-10,
            "frozen at step {step} should stay 20 (the t=0 value of growing[a2]), got {val}"
        );
    }
}
