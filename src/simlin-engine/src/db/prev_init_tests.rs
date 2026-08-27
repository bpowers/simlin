// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;

/// The complete snapshot-intrinsic alphabet used by shared refusal tests.
/// Iterating this table keeps the `PREVIOUS` and `INIT` lowering branches from
/// acquiring one-sided coverage as their surface syntax evolves.
#[derive(Clone, Copy, Debug)]
pub(super) enum SnapshotBuiltin {
    Previous,
    Init,
}

impl SnapshotBuiltin {
    pub(super) const ALL: [SnapshotBuiltin; 2] = [SnapshotBuiltin::Previous, SnapshotBuiltin::Init];

    pub(super) const fn name(self) -> &'static str {
        match self {
            SnapshotBuiltin::Previous => "PREVIOUS",
            SnapshotBuiltin::Init => "INIT",
        }
    }

    pub(super) fn call(self, argument: &str) -> String {
        match self {
            SnapshotBuiltin::Previous => format!("PREVIOUS({argument}, -7)"),
            SnapshotBuiltin::Init => format!("INIT({argument})"),
        }
    }

    fn unaddressable_argument_message(self) -> &'static str {
        match self {
            SnapshotBuiltin::Previous => {
                "PREVIOUS requires a variable reference after helper rewriting"
            }
            SnapshotBuiltin::Init => "INIT requires a variable reference argument",
        }
    }
}

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
    let mut implicit_vars: Vec<crate::capture::ImplicitVar> = Vec::new();
    let parsed: crate::variable::Variable<datamodel::ModuleReference, crate::ast::Expr0> =
        crate::variable::parse_var(
            &crate::variable::ParseContext::new(
                &crate::dimensions::DimensionsContext::from(dims.as_slice()),
                &units_ctx,
            ),
            &aux,
            &mut implicit_vars,
            |mi| Ok(Some(mi.clone())),
        );
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
            .map(|v| v.ident().to_string())
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
    let mut implicit_vars: Vec<crate::capture::ImplicitVar> = Vec::new();
    let parsed: crate::variable::Variable<datamodel::ModuleReference, crate::ast::Expr0> =
        crate::variable::parse_var(
            &crate::variable::ParseContext::new(
                &crate::dimensions::DimensionsContext::from(dims.as_slice()),
                &units_ctx,
            ),
            &aux,
            &mut implicit_vars,
            |mi| Ok(Some(mi.clone())),
        );
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
            .map(|v| v.ident().to_string())
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

/// The complete bare-element classification at the remaining generated-LTM
/// parse boundary: an unshadowed element is static, a same-named variable is
/// conservative, and an ordinary source parse has no model-name input and is
/// conservative too. The model name sets and source equation are obtained
/// through the same sync/query functions production uses.
#[test]
fn bare_element_snapshot_shadowing_has_all_three_production_rows() {
    use crate::db::ltm::{LtmEquation, ltm_model_var_names, parse_ltm_equation};
    use crate::test_common::TestProject;

    let classify = |shadowed: bool| {
        let mut tp = TestProject::new("bare_element_shadowing")
            .named_dimension("DimA", &["a1", "b2"])
            .named_dimension("DimB", &["b2", "x1"])
            .array_aux("base_val[DimA]", "1")
            .aux("lagged", "PREVIOUS(base_val[b2], 0)", None);
        if shadowed {
            tp = tp.aux("b2", "1", None);
        }
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &tp.build_datamodel());
        let model = sync.models["main"].source;
        let source_var = sync.models["main"].variables["lagged"].source;
        let source_text = match source_var.equation(&db) {
            datamodel::Equation::Scalar(text) => text.clone(),
            _ => unreachable!("the production fixture declares a scalar equation"),
        };
        let ltm_equation = LtmEquation::scalar(source_text);
        let ltm = parse_ltm_equation(
            "lagged",
            &ltm_equation,
            project_dimensions_context(&db, sync.project),
            Some(ltm_model_var_names(&db, model, sync.project)),
        );
        let source = parse_source_variable(&db, source_var, sync.project);
        (ltm.implicit_vars.len(), source.implicit_vars.len())
    };

    assert_eq!(classify(false), (0, 1), "unshadowed LTM / source rows");
    assert_eq!(classify(true), (1, 1), "shadowed LTM / source rows");
}

/// Qualified module output ports name ordinary stored values after lowering.
/// The fixture makes the module block's first slot, the selected scalar port,
/// and both array elements distinct so a read from slot zero or element zero
/// cannot accidentally satisfy the value assertions.
#[test]
fn scalar_and_array_module_output_snapshots_need_no_capture() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let array_output = datamodel::Variable::Aux(datamodel::Aux {
        ident: "z_array_output".to_string(),
        equation: datamodel::Equation::ApplyToAll(
            vec!["d".to_string()],
            "100 * d + TIME".to_string(),
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let producer = x_model(
        "producer",
        vec![
            x_aux("a_padding", "901", None),
            x_aux("z_output", "10 + TIME", None),
            array_output,
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_module("producer", &[], None),
            x_aux("prev_scalar", "PREVIOUS(producer.z_output, -7)", None),
            x_aux("init_scalar", "INIT(producer.z_output)", None),
            x_aux(
                "prev_array_element",
                "PREVIOUS(producer.z_array_output[2], -7)",
                None,
            ),
            x_aux(
                "init_array_element",
                "INIT(producer.z_array_output[2])",
                None,
            ),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 3.0;
    specs.dt = datamodel::Dt::Dt(1.0);
    let mut project = x_project(specs, &[main, producer]);
    project.dimensions.push(datamodel::Dimension::named(
        "d".to_string(),
        vec!["e1".to_string(), "e2".to_string()],
    ));

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    for name in [
        "prev_scalar",
        "init_scalar",
        "prev_array_element",
        "init_array_element",
    ] {
        let source = sync.models["main"].variables[name].source;
        assert!(
            parse_source_variable(&db, source, sync.project)
                .implicit_vars
                .iter()
                .all(|helper| helper.capture().is_none()),
            "{name} must not synthesize a redundant capture"
        );
    }
    let dep_graph = model_dependency_graph(
        &db,
        sync.models["main"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    let producer_initial = dep_graph
        .runlist_initials
        .iter()
        .position(|name| name == "producer")
        .expect("INIT(producer.z_output) must seed the module's initials evaluation");
    let reader_initial = dep_graph
        .runlist_initials
        .iter()
        .position(|name| name == "init_scalar")
        .expect("the INIT reader must be in the initials runlist");
    assert!(
        producer_initial < reader_initial,
        "the module must initialize before its qualified-output reader: {:?}",
        dep_graph.runlist_initials
    );
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("qualified scalar and array-element module ports must compile");
    let mut vm = crate::vm::Vm::new(compiled).expect("the qualified-port model must build a VM");
    vm.run_to_end()
        .expect("the qualified-port model must simulate");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["prev_scalar"], [-7.0, 10.0, 11.0, 12.0]);
    assert_eq!(results["prev_array_element"], [-7.0, 200.0, 201.0, 202.0]);
    assert_eq!(results["init_scalar"], [10.0, 10.0, 10.0, 10.0]);
    assert_eq!(results["init_array_element"], [200.0, 200.0, 200.0, 200.0]);
}

/// A qualified output can traverse more than one module boundary. Every
/// component is resolved structurally before the snapshot slot is selected;
/// the padding values make reading either enclosing block's first slot visible.
#[test]
fn nested_qualified_module_output_snapshots_read_the_leaf_port() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module_named, x_project};

    let leaf = x_model(
        "leaf",
        vec![
            x_aux("a_padding", "801", None),
            x_aux("z_port", "50 + TIME", None),
        ],
    );
    let middle = x_model(
        "middle",
        vec![
            x_aux("a_padding", "701", None),
            x_module_named("n", "leaf", &[], None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_module_named("m", "middle", &[], None),
            x_aux("lagged", "PREVIOUS(m.n.z_port, -7)", None),
            x_aux("frozen", "INIT(m.n.z_port)", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 3.0;
    specs.dt = datamodel::Dt::Dt(1.0);
    let project = x_project(specs, &[main, middle, leaf]);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let lagged = sync.models["main"].variables["lagged"].source;
    assert!(
        parse_source_variable(&db, lagged, sync.project)
            .implicit_vars
            .iter()
            .all(|helper| helper.capture().is_none()),
        "a nested qualified output is resolved by lowering, without a parse capture"
    );
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the nested qualified output must compile");
    let mut vm = crate::vm::Vm::new(compiled).expect("the nested module model must build a VM");
    vm.run_to_end().expect("the nested module model must run");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["lagged"], [-7.0, 50.0, 51.0, 52.0]);
    assert_eq!(results["frozen"], [50.0, 50.0, 50.0, 50.0]);
}

/// Child initialization uses the child's initial AST, not its flow AST, and a
/// stock output remains a normal initial-phase value. These two rows prevent a
/// runtime implementation from evaluating child flows during parent initials.
#[test]
fn qualified_init_uses_child_active_initial_and_stock_values() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project, x_stock};

    let mut active_output = x_aux("active_output", "10 + TIME", None);
    if let datamodel::Variable::Aux(output) = &mut active_output {
        output.compat.active_initial = Some("55".to_string());
    } else {
        unreachable!("x_aux constructs an aux");
    }
    let child = x_model(
        "child",
        vec![
            x_aux("padding", "901", None),
            active_output,
            x_stock("level", "33", &[], &[], None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_module("child", &[], None),
            x_aux("frozen_active", "INIT(child.active_output)", None),
            x_aux("frozen_stock", "INIT(child.level)", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 2.0;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &x_project(specs, &[main, child]));
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    let active_offset = vm
        .get_offset(&Ident::new("frozen_active"))
        .expect("frozen_active result offset");
    let stock_offset = vm
        .get_offset(&Ident::new("frozen_stock"))
        .expect("frozen_stock result offset");

    vm.run_initials().expect("initials run");
    assert_eq!(vm.get_value_now(active_offset), 55.0);
    assert_eq!(vm.get_value_now(stock_offset), 33.0);

    vm.reset();
    vm.run_initials().expect("initials rerun after reset");
    assert_eq!(vm.get_value_now(active_offset), 55.0);
    assert_eq!(vm.get_value_now(stock_offset), 33.0);

    vm.run_to_end().expect("runs");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["frozen_active"], [55.0, 55.0, 55.0]);
    assert_eq!(results["frozen_stock"], [33.0, 33.0, 33.0]);
}

/// Two instances with the same `(model, input-set)` share one compiled module,
/// so their demanded outputs are unioned on that key. The unrelated output
/// stays absent: cross-model initialization remains sparse.
#[test]
fn qualified_init_unions_same_module_key_without_initializing_unrelated_outputs() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module_named, x_project};

    let child = x_model(
        "child",
        vec![
            x_aux("out_a", "10", None),
            x_aux("out_b", "20", None),
            x_aux("unrelated", "999", None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_module_named("first", "child", &[], None),
            x_module_named("second", "child", &[], None),
            x_aux("frozen_a", "INIT(first.out_a)", None),
            x_aux("frozen_b", "INIT(second.out_b)", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 1.0;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &x_project(specs, &[main, child]));
    let child_graph = model_dependency_graph(
        &db,
        sync.models["child"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert!(
        child_graph
            .runlist_initials
            .iter()
            .any(|name| name == "out_a")
    );
    assert!(
        child_graph
            .runlist_initials
            .iter()
            .any(|name| name == "out_b")
    );
    assert!(
        !child_graph
            .runlist_initials
            .iter()
            .any(|name| name == "unrelated"),
        "only outputs reached by an actual caller initial dependency are seeded: {:?}",
        child_graph.runlist_initials
    );

    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["frozen_a"], [10.0, 10.0]);
    assert_eq!(results["frozen_b"], [20.0, 20.0]);
}

/// The runtime compilation key is shared across project roots as well as
/// across instances within one root. Requirement discovery must therefore
/// remain project-wide even though its dependency-graph reads are pure: each
/// root below demands a different output from the same no-input child key.
#[test]
fn qualified_init_unions_same_module_key_across_project_roots() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let root_a = x_model(
        "root_a",
        vec![
            x_module("child", &[], None),
            x_aux("frozen_a", "INIT(child.out_a)", None),
        ],
    );
    let root_b = x_model(
        "root_b",
        vec![
            x_module("child", &[], None),
            x_aux("frozen_b", "INIT(child.out_b)", None),
        ],
    );
    let child = x_model(
        "child",
        vec![
            x_aux("out_a", "10", None),
            x_aux("out_b", "20", None),
            x_aux("unrelated", "999", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 1.0;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &x_project(specs, &[root_a, root_b, child]));

    let child_graph = model_dependency_graph(
        &db,
        sync.models["child"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert!(
        child_graph
            .runlist_initials
            .iter()
            .any(|name| name == "out_a")
    );
    assert!(
        child_graph
            .runlist_initials
            .iter()
            .any(|name| name == "out_b")
    );
    assert!(
        !child_graph
            .runlist_initials
            .iter()
            .any(|name| name == "unrelated"),
        "the cross-root union must remain sparse: {:?}",
        child_graph.runlist_initials
    );

    for (root, output, expected) in [
        ("root_a", "frozen_a", [10.0, 10.0]),
        ("root_b", "frozen_b", [20.0, 20.0]),
    ] {
        let compiled = compile_project_incremental(&db, sync.project, root)
            .unwrap_or_else(|error| panic!("{root} compiles: {error:?}"));
        let mut vm = crate::vm::Vm::new(compiled).expect("root VM");
        vm.run_to_end().expect("root simulation");
        let results = crate::test_common::collect_results(&vm.into_results());
        assert_eq!(
            results[output], expected,
            "{root} reads its demanded output"
        );
    }
}

/// Bound-port sets are part of a compiled module's identity. Distinct keys
/// receive distinct output seeds and pull in only the selected output's local
/// dependency closure.
#[test]
fn qualified_init_requirements_are_keyed_by_module_input_set() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module_named, x_project};

    let module_input = |name: &str| {
        let mut input = x_aux(name, "0", None);
        let datamodel::Variable::Aux(input) = &mut input else {
            unreachable!("x_aux constructs an aux")
        };
        input.compat.can_be_module_input = true;
        datamodel::Variable::Aux(input.clone())
    };
    let child = x_model(
        "child",
        vec![
            module_input("input_a"),
            module_input("input_b"),
            x_aux("out_a", "input_a * 10", None),
            x_aux("out_b", "input_b * 100", None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("source_a", "3", None),
            x_aux("source_b", "4", None),
            x_module_named(
                "one_port",
                "child",
                &[("source_a", "one_port.input_a")],
                None,
            ),
            x_module_named(
                "two_ports",
                "child",
                &[
                    ("source_a", "two_ports.input_a"),
                    ("source_b", "two_ports.input_b"),
                ],
                None,
            ),
            x_aux("frozen_a", "INIT(one_port.out_a)", None),
            x_aux("frozen_b", "INIT(two_ports.out_b)", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 1.0;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &x_project(specs, &[main, child]));
    let child_model = sync.models["child"].source;
    let input_sets = crate::db::assemble::module_input_sets_for(&db, sync.project, "main", "child");
    assert_eq!(
        input_sets.len(),
        2,
        "fixture must produce two compilation keys"
    );
    for inputs in input_sets {
        let graph = model_dependency_graph(
            &db,
            child_model,
            sync.project,
            ModuleInputSet::from_canonical_set(&db, &inputs),
        );
        let has_a = graph.runlist_initials.iter().any(|name| name == "out_a");
        let has_b = graph.runlist_initials.iter().any(|name| name == "out_b");
        if inputs.contains("input_b") {
            assert_eq!((has_a, has_b), (false, true), "two-port key: {graph:?}");
            let input_pos = graph
                .runlist_initials
                .iter()
                .position(|name| name == "input_b")
                .expect("out_b's input closure");
            let output_pos = graph
                .runlist_initials
                .iter()
                .position(|name| name == "out_b")
                .expect("out_b seed");
            assert!(input_pos < output_pos);
        } else {
            assert_eq!((has_a, has_b), (true, false), "one-port key: {graph:?}");
            let input_pos = graph
                .runlist_initials
                .iter()
                .position(|name| name == "input_a")
                .expect("out_a's input closure");
            let output_pos = graph
                .runlist_initials
                .iter()
                .position(|name| name == "out_a")
                .expect("out_a seed");
            assert!(input_pos < output_pos);
        }
    }

    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["frozen_a"], [30.0, 30.0]);
    assert_eq!(results["frozen_b"], [400.0, 400.0]);
}

/// An externally seeded output can depend on a generated stdlib module. The
/// fixed point follows that implicit module boundary and initializes its
/// output before the enclosing model publishes `smoothed` to its caller.
#[test]
fn qualified_init_follows_an_implicit_module_dependency() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let child = x_model(
        "child",
        vec![
            {
                let mut input = x_aux("input", "0", None);
                let datamodel::Variable::Aux(input) = &mut input else {
                    unreachable!("x_aux constructs an aux")
                };
                input.compat.can_be_module_input = true;
                datamodel::Variable::Aux(input.clone())
            },
            x_aux("smoothed", "SMTH1(input, 2)", None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("source", "8", None),
            x_module("child", &[("source", "child.input")], None),
            x_aux("frozen", "INIT(child.smoothed)", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 2.0;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &x_project(specs, &[main, child]));
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["frozen"], [8.0, 8.0, 8.0]);
}

/// PREVIOUS is lagged in both dependency phases. A qualified PREVIOUS-only
/// reader must not seed the child output in initials; the explicit fallback is
/// the t0 value and the child flow value is available from the next step.
#[test]
fn qualified_previous_only_does_not_seed_child_initials() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let child = x_model("child", vec![x_aux("output", "10 + TIME", None)]);
    let main = x_model(
        "main",
        vec![
            x_module("child", &[], None),
            x_aux("lagged", "PREVIOUS(child.output, -7)", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 2.0;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &x_project(specs, &[main, child]));
    let graph = model_dependency_graph(
        &db,
        sync.models["child"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert!(
        !graph.runlist_initials.iter().any(|name| name == "output"),
        "PREVIOUS-only qualified reads are filtered before propagation: {:?}",
        graph.runlist_initials
    );
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["lagged"], [-7.0, 10.0, 11.0]);
}

/// Cross-model seeds enter the same resolved-SCC ordering path as model-local
/// initial seeds. This forward element recurrence is a one-member resolved SCC
/// and must publish its final element during parent initialization.
#[test]
fn qualified_init_seed_preserves_resolved_scc_initial_order() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let recurrence = datamodel::Variable::Aux(datamodel::Aux {
        ident: "series".to_string(),
        equation: datamodel::Equation::Arrayed(
            vec!["d".to_string()],
            vec![
                ("d1".to_string(), "1".to_string(), None, None),
                ("d2".to_string(), "series[d1] + 1".to_string(), None, None),
                ("d3".to_string(), "series[d2] + 1".to_string(), None, None),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let child = x_model("child", vec![recurrence]);
    let main = x_model(
        "main",
        vec![
            x_module("child", &[], None),
            x_aux("frozen", "INIT(child.series[d3])", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 1.0;
    let mut project = x_project(specs, &[main, child]);
    project.dimensions.push(datamodel::Dimension::named(
        "d".to_string(),
        vec!["d1".to_string(), "d2".to_string(), "d3".to_string()],
    ));
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let child_graph = model_dependency_graph(
        &db,
        sync.models["child"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert_eq!(child_graph.resolved_sccs.len(), 1);
    assert_eq!(child_graph.runlist_initials, ["series"]);
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(results["frozen"], [3.0, 3.0]);
}

/// A bare module instance denotes a whole slot block, never one snapshot slot.
/// The source parse is context-free and therefore leaves the reference intact;
/// lowering, which has the dependency shape, must reject it rather than read
/// the block's first slot.
#[test]
fn direct_snapshot_of_a_bare_module_refuses_both_intrinsics_loudly() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    for builtin in SnapshotBuiltin::ALL {
        let target = format!("{}_bare_module", builtin.name().to_lowercase());
        let main = x_model(
            "main",
            vec![
                x_module("producer", &[], None),
                x_aux(&target, &builtin.call("producer"), None),
            ],
        );
        let producer = x_model("producer", vec![x_aux("output", "TIME", None)]);
        let project = x_project(sim_specs_with_units("month"), &[main, producer]);
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);

        let err = compile_project_incremental(&db, sync.project, "main")
            .expect_err("a bare module snapshot must not compile as slot zero");
        assert_eq!(err.code, crate::common::ErrorCode::NotSimulatable);
        let diagnostics = collect_all_diagnostics(&db, sync.project);
        assert!(
            diagnostics.iter().any(|diag| {
                diag.model == "main"
                    && diag.variable.as_deref() == Some(target.as_str())
                    && diag.severity == DiagnosticSeverity::Error
                    && matches!(
                        &diag.error,
                        DiagnosticError::Equation(error)
                            if error.code == crate::common::ErrorCode::NotSimulatable
                                && error.details.as_deref().is_some_and(|details| details.contains(
                                    "cannot read the bare module instance 'producer'"
                                ))
                    )
            }),
            "{} refusal must be attributed to main/{target}, got {diagnostics:?}",
            builtin.name()
        );
    }
}

/// Whether an aux is bound as a module input is an instantiation property, so
/// it cannot affect the source parse. The no-capture shape is pinned here; the
/// current lowering refusal is loud because transient module inputs have no
/// snapshot storage of their own.
#[test]
fn bound_module_input_snapshot_refuses_both_intrinsics_loudly() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let input = datamodel::Variable::Aux(datamodel::Aux {
        ident: "input".to_string(),
        equation: datamodel::Equation::Scalar("0".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat {
            can_be_module_input: true,
            ..datamodel::Compat::default()
        },
    });
    for builtin in SnapshotBuiltin::ALL {
        let target = format!("{}_input", builtin.name().to_lowercase());
        let sub = x_model(
            "sub",
            vec![input.clone(), x_aux(&target, &builtin.call("input"), None)],
        );
        let main = x_model(
            "main",
            vec![
                x_aux("source", "TIME", None),
                x_module("sub", &[("source", "sub.input")], None),
                x_aux("out", &format!("sub.{target}"), None),
            ],
        );
        let project = x_project(sim_specs_with_units("month"), &[main, sub]);
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let target_var = sync.models["sub"].variables[target.as_str()].source;
        assert!(
            parse_source_variable(&db, target_var, sync.project)
                .implicit_vars
                .iter()
                .all(|helper| helper.capture().is_none()),
            "binding another model must not change the {} source parse",
            builtin.name()
        );
        let err = compile_project_incremental(&db, sync.project, "main")
            .expect_err("a transient module input has no snapshot to address");
        assert_eq!(err.code, crate::common::ErrorCode::NotSimulatable);
        let input_sets =
            crate::db::assemble::module_input_sets_for(&db, sync.project, "main", "sub");
        assert_eq!(input_sets.len(), 1, "the fixture has one sub instance");
        let inputs = ModuleInputSet::from_canonical_set(&db, &input_sets[0]);
        let sub_model = sync.models["sub"].source;
        let diagnostics = compile_var_fragment::accumulated::<CompilationDiagnostic>(
            &db,
            target_var,
            sub_model,
            sync.project,
            inputs,
        );
        assert!(
            diagnostics.iter().any(|CompilationDiagnostic(diag)| {
                diag.model == "sub"
                    && diag.variable.as_deref() == Some(target.as_str())
                    && diag.severity == DiagnosticSeverity::Error
                    && matches!(
                        &diag.error,
                        DiagnosticError::Assembly(message)
                            if message.contains(builtin.unaddressable_argument_message())
                    )
            }),
            "{} refusal must be attributed to sub/{target}, got {diagnostics:?}",
            builtin.name()
        );
    }
}

/// End-to-end (salsa + VM): an LTM-instrumented model whose A2A target
/// references arrayed deps with bare element subscripts (declared by multiple
/// dimensions) compiles its LTM link scores without synthesizing any helper
/// auxes, and produces the same simulation values either way.
#[test]
fn test_ltm_bare_element_subscripts_no_helpers() {
    use salsa::Setter;

    use crate::db::{model_ltm_implicit_var_info, model_ltm_variables};
    use crate::test_common::TestProject;

    // `b2` is in both DimA and DimB at different positions. The model's
    // equations reference `base[b2]` (FixedIndex with a bare element) and
    // `other[DimA]` (the A2A iterated form).
    let tp = TestProject::new("ltm_bare_elem_no_helpers")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("DimA", &["a1", "b2"])
        .named_dimension("DimB", &["b2", "x1"])
        .array_stock("pop[DimA]", "100", &["grow"], &[], None)
        .array_flow("grow[DimA]", "pop[DimA] * rate[b2] * other[DimA]", None)
        .array_aux("rate[DimA]", "0.01 + pop[DimA] / 10000")
        .array_aux("other[DimA]", "1 + pop[b2] / 1000");

    tp.assert_compiles_incremental();

    let db = crate::db::SimlinDb::default();
    let project = tp.build_datamodel();
    let sync = crate::db::sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let mut db = db;
    sync.project.set_ltm_enabled(&mut db).to(true);
    sync.project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    assert!(
        !ltm_vars.vars.is_empty(),
        "LTM discovery must emit link scores for this model"
    );

    let info = model_ltm_implicit_var_info(&db, source_model, sync.project);
    // The only helpers allowed are the flow-to-stock link score's nested
    // PREVIOUS(PREVIOUS(...)) captures (semantically necessary: the VM keeps
    // one step of history, so a two-step lag needs a helper that re-lags a
    // lagged value). Bare-element subscripts (`rate[b2]`, `pop[b2]`) must not
    // synthesize any.
    let non_flow_to_stock: Vec<&String> = info
        .keys()
        .filter(|name| !name.contains("grow\u{2192}pop"))
        .collect();
    assert!(
        non_flow_to_stock.is_empty(),
        "only the flow-to-stock nested-PREVIOUS helpers may remain; unexpected: {non_flow_to_stock:?}"
    );
}

use crate::snapshot_arg::SnapshotAccess;

/// One `PREVIOUS`/`INIT` argument shape, and what each of the two decisions
/// `snapshot_arg::SnapshotArg::access` states for it.
struct AgreementRow {
    /// Which source-classification and lowered-access arms this row covers, so
    /// the table can be read back against both representations.
    covers: &'static str,
    /// True when the equation is apply-to-all over `d` (`out[d] = ..`), false
    /// when it is a scalar aux (`lagged = ..`).
    a2a: bool,
    equation: &'static str,
    /// Does the parse synthesize a capture helper for the argument?
    captures: bool,
    /// What codegen makes of the argument AS WRITTEN: the lowered form of that
    /// argument, which for a capture row is the helper's own lowered body --
    /// the helper equation IS the argument. `None` when the model does not
    /// compile at all, so codegen never classifies anything.
    codegen: Option<SnapshotAccess>,
    /// Why the two differ, when they do. Every one is the parse being STRICTER
    /// than codegen (a capture where a direct read would have worked) except
    /// where the note says otherwise; none is a direct read codegen refuses to
    /// address, which is the direction that would produce wrong numbers.
    divergence: Option<&'static str>,
}

/// The parse's capture decision and codegen's direct-read decision are one
/// rule ([`crate::snapshot_arg::SnapshotArg::access`]), and this table is what
/// says so shape by shape.
///
/// Rows are derived from both representations that apply the shared rule:
///
/// * the parse's `BuiltinVisitor::snapshot_arg` -- a `Var` whose identity is
///   known storage or known non-storage, a `Subscript` whose every index is
///   static / leaves a dimension standing / is dynamic, and the catch-all for
///   anything that is not a reference;
/// * codegen's, `static_slot` plus `Compiler::snapshot_static_view` -- an
///   `Expr::Var`, a `StaticSubscript` that collapsed to one element, a
///   `StaticSubscript` with dimensions standing, and the refusals.
///
/// Every fixture goes through production: the capture decision is read from
/// `model_implicit_var_info` (the parse the compiler actually memoizes) and
/// the classification from `db::var_fragment::explicit_fragment_input` /
/// `db::fragment_compile::implicit_fragment_input` plus
/// `compiler::fragment::lower_fragment` (the lowering codegen actually
/// receives). Nothing here hand-builds an argument.
///
/// **Arms this table does not cover, and where they are covered instead.**
///
/// * Qualified scalar and array-element module outputs are production-built
///   and value-pinned by
///   `scalar_and_array_module_output_snapshots_need_no_capture`; bound module
///   inputs and bare modules are the non-storage lowering rows pinned for both
///   intrinsics by the two refusal-table tests above and the LTM twin.
/// * An index that is BOTH `SpansDimension` and `Static` -- a name that is an
///   active apply-to-all dimension and also an element of some other
///   dimension. No corpus model reaches it: the branch of `index_is_static`
///   that accepts a bare name at all needs the model's variable-name set,
///   which only the LTM parse supplies, so only an LTM synthetic whose
///   iterated dimension's name is also another dimension's element could
///   (`ltm_augment::wrap_non_matching_in_previous` wraps such a reference
///   without normalizing it); spans-first is what the replaced code did. The
///   precedence is stated once in `SnapshotArg::subscripted` and pinned by its
///   own test there, over the classified-index alphabet that is that
///   function's actual domain.
/// * Lowered temporary arrays and position-specific view refusals are not
///   source-classification rows. Their typed-error propagation is covered by
///   `compiler::codegen::tests::previous_of_non_var_inside_subscript_index_is_err_not_panic`
///   and its array-view twin; this table instead classifies each expression
///   that production parsing and fragment lowering supply here.
#[test]
fn every_prev_init_argument_shape_agrees_between_the_parse_and_codegen() {
    use crate::compiler::fragment::lower_fragment;
    use crate::compiler::{BuiltinFn, Expr, lowered_snapshot_arg};
    use crate::db::fragment_compile::implicit_fragment_input;
    use crate::test_common::TestProject;

    let rows = [
        AgreementRow {
            covers: "visitor: Var, base not module-backed. codegen: Expr::Var",
            a2a: false,
            equation: "PREVIOUS(k, 0)",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: Var, scalar module-call aux. codegen: Expr::Var",
            a2a: false,
            equation: "PREVIOUS(smoothed, 0)",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: Subscript, numeric index. codegen: collapsed StaticSubscript",
            a2a: false,
            equation: "PREVIOUS(vals[2], 0)",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: Subscript, qualified dimension.element, SCALAR parse. \
                     codegen: collapsed StaticSubscript",
            a2a: false,
            equation: "PREVIOUS(vals[d.e2], 0)",
            captures: true,
            codegen: Some(SnapshotAccess::Slot),
            divergence: Some(
                "a qualified `dimension.element` index folds to a constant regardless of \
                 context, but the branch of `index_is_static` that recognizes one needs \
                 the qualified dimension in `dimensions_ctx`, and the parse narrows that \
                 context to the variable's own relevant dimensions and their map chains \
                 -- empty for a SCALAR variable (a dimension edit must not re-parse every \
                 unrelated variable). So the same argument captures here and does not in \
                 the apply-to-all row below, whose own dimension is the qualified one",
            ),
        },
        AgreementRow {
            covers: "visitor: Subscript, qualified dimension.element, A2A parse. \
                     codegen: collapsed StaticSubscript",
            a2a: true,
            equation: "PREVIOUS(vals[d.e2], 0)",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: Subscript, bare element name. codegen: collapsed StaticSubscript",
            a2a: false,
            equation: "PREVIOUS(vals[e1], 0)",
            captures: true,
            codegen: Some(SnapshotAccess::Slot),
            divergence: Some(
                "XMILE lets an element name shadow a variable name, so without the \
                 model's variable-name set the parse cannot tell `vals[e1]` from a \
                 dynamic index through a variable named `e1` and captures \
                 conservatively. Lowering knows `vals`' declared dimensions and \
                 resolves the element, so codegen sees a fixed slot. Generalizing the \
                 LTM path's rule to user equations removes the capture and changes the \
                 artifact",
            ),
        },
        AgreementRow {
            covers: "visitor: Subscript, dynamic index. codegen: Expr::Subscript",
            a2a: false,
            equation: "PREVIOUS(vals[idx], 0)",
            captures: true,
            codegen: Some(SnapshotAccess::Capture),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: Subscript, index leaves a dimension standing. \
                     codegen: StaticSubscript with dims",
            a2a: true,
            equation: "VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)",
            captures: false,
            codegen: Some(SnapshotAccess::View),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: Subscript over the iterated dimension in a SCALAR position, \
                     substituted per element. codegen: collapsed StaticSubscript",
            a2a: true,
            equation: "PREVIOUS(vals[d], 0)",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: the catch-all, an argument that is no reference at all. \
                     codegen: Expr::Op2",
            a2a: false,
            equation: "PREVIOUS(k * 2, 0)",
            captures: true,
            codegen: Some(SnapshotAccess::Capture),
            divergence: None,
        },
        AgreementRow {
            covers: "INIT twin of the bare-variable row",
            a2a: false,
            equation: "INIT(k)",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "INIT twin of the catch-all row",
            a2a: false,
            equation: "INIT(k * 2)",
            captures: true,
            codegen: Some(SnapshotAccess::Capture),
            divergence: None,
        },
        AgreementRow {
            covers: "INIT twin of the per-element A2A row",
            a2a: true,
            equation: "INIT(vals[d])",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "visitor: Var, base not module-backed but ARRAYED. codegen: refuses",
            a2a: false,
            equation: "PREVIOUS(vals, 0)",
            captures: false,
            codegen: None,
            divergence: Some(
                "the only row where the parse is the LOOSER of the two, and it is a \
                 difference of information rather than of rule: over `Expr0` a bare \
                 name has no arity, so the parse calls it whole storage, while lowering \
                 knows `vals` is three slots and codegen refuses an array-valued \
                 PREVIOUS in a scalar position. The equation is ill-typed with or \
                 without the PREVIOUS -- `lagged = vals` refuses too, with `an array of \
                 shape [3] is used where a single value is required` -- so the refusal \
                 is loud, no artifact exists to differ, and closing it means giving the \
                 decision the dimensions, which is what moving it to lowering does",
            ),
        },
    ];

    // Every argument of every `PREVIOUS`/`INIT` reachable from one lowered
    // expression, in walk order.
    fn snapshot_args<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
        match expr {
            Expr::App(builtin, _) => {
                if let BuiltinFn::Previous(arg, _) | BuiltinFn::Init(arg) = builtin {
                    out.push(arg.as_ref());
                }
                for arg in builtin.args() {
                    snapshot_args(arg, out);
                }
            }
            Expr::Op1(_, r, _) => snapshot_args(r, out),
            Expr::Op2(_, l, r, _) => {
                snapshot_args(l, out);
                snapshot_args(r, out);
            }
            Expr::If(c, t, f, _) => {
                snapshot_args(c, out);
                snapshot_args(t, out);
                snapshot_args(f, out);
            }
            Expr::AssignCurr(_, inner)
            | Expr::AssignNext(_, inner)
            | Expr::AssignTemp(_, inner, _) => snapshot_args(inner, out),
            _ => {}
        }
    }

    for row in &rows {
        let target = if row.a2a { "out" } else { "lagged" };
        let tp = {
            let base = TestProject::new("prev_init_agreement")
                .with_sim_time(0.0, 2.0, 1.0)
                .named_dimension("d", &["e1", "e2", "e3"])
                .array_with_ranges("vals[d]", vec![("e1", "30"), ("e2", "10"), ("e3", "20")])
                .scalar_aux("k", "3")
                .aux("idx", "1 + MIN(TIME, 1)", None)
                .aux("smoothed", "SMTH1(k, 2)", None);
            if row.a2a {
                base.array_aux("out[d]", row.equation)
            } else {
                base.aux(target, row.equation, None)
            }
        };
        let what = row.covers;
        let eqn = row.equation;

        // The parse the compiler memoizes, not a re-derivation.
        let dm = tp.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &dm);
        let model = sync.models["main"].source;
        let target_var = model.variables(&db)[target];
        let helpers: Vec<&ImplicitVarMeta> = model_implicit_var_info(&db, model, sync.project)
            .values()
            .filter(|meta| meta.parent_source_var == target_var)
            .collect();
        assert_eq!(
            !helpers.is_empty(),
            row.captures,
            "{what}: `{eqn}` -- capture-helper expectation"
        );

        // Codegen's classification of the SAME argument. For a capture row the
        // argument is the helper's equation, so lowering the helper is what
        // shows the shape codegen would have been handed without the capture.
        let observed: Vec<SnapshotAccess> = if row.captures {
            helpers
                .iter()
                .flat_map(|meta| {
                    let input = implicit_fragment_input(&db, meta, model, sync.project, &[])
                        .unwrap_or_else(|_| panic!("{what}: helper must build a fragment input"));
                    lower_fragment(&input, false)
                        .unwrap_or_else(|_| panic!("{what}: helper must lower"))
                        .ast
                        .iter()
                        .map(|expr| match expr {
                            Expr::AssignCurr(_, inner) => lowered_snapshot_arg(inner).access(),
                            other => lowered_snapshot_arg(other).access(),
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        } else if row.codegen.is_none() {
            // The model refuses, so nothing is lowered to classify; the
            // refusal itself is the assertion.
            assert!(
                tp.compile_incremental().is_err(),
                "{what}: `{eqn}` -- the row says the model does not compile"
            );
            Vec::new()
        } else {
            let mut found = Vec::new();
            let exprs = crate::db::var_noninitial_lowered_exprs(&db, model, sync.project, target);
            for expr in &exprs {
                snapshot_args(expr, &mut found);
            }
            assert!(
                !found.is_empty(),
                "{what}: `{eqn}` -- no PREVIOUS/INIT survived lowering to classify"
            );
            found
                .iter()
                .map(|arg| lowered_snapshot_arg(arg).access())
                .collect()
        };
        if let Some(expected) = row.codegen {
            assert!(
                !observed.is_empty() && observed.iter().all(|a| *a == expected),
                "{what}: `{eqn}` -- codegen classification, expected all {expected:?}, \
                 got {observed:?}"
            );
        }

        // The agreement itself, in both directions: a synthesized capture's
        // argument is one codegen would not have addressed, and a direct read
        // is one it does address. The exceptions are enumerated, not hidden.
        match (row.codegen, row.divergence) {
            // Codegen classified the argument, so the two verdicts compare.
            (Some(codegen), None) => assert_eq!(
                row.captures,
                codegen == SnapshotAccess::Capture,
                "{what}: `{eqn}` -- the parse and codegen must agree on a row with no \
                 recorded divergence"
            ),
            (Some(codegen), Some(why)) => assert_ne!(
                row.captures,
                codegen == SnapshotAccess::Capture,
                "{what}: `{eqn}` -- this row records a divergence that no longer \
                 exists, so the record is stale: {why}"
            ),
            // The model does not compile, so there is no classification to
            // compare: the divergence IS the refusal, which the compilability
            // assertion below pins. Such a row must not also capture -- a
            // capture would have handed codegen a slot and it would compile.
            (None, Some(_)) => assert!(
                !row.captures,
                "{what}: `{eqn}` -- a refusing row cannot also synthesize a capture"
            ),
            (None, None) => {
                panic!("{what}: `{eqn}` -- a row whose model does not compile must record why")
            }
        }

        // Whatever the parse produced, codegen accepted it: no row leaves a
        // PREVIOUS/INIT that the emitter refuses, except the one that says so.
        assert_eq!(
            tp.compile_incremental().is_ok(),
            row.codegen.is_some(),
            "{what}: `{eqn}` -- compilability"
        );
    }
}
