// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::capture::CaptureKind;
use crate::datamodel;
use crate::db::dep_graph::{RunlistMembership, implicit_var_runlist_membership};
use crate::test_common::TestProject;

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

/// The two snapshot intrinsics. A test that derives its rows from this list
/// covers both opcode paths (`LoadPrev` and `LoadInitial`) instead of pinning
/// one and assuming the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotBuiltin {
    Previous,
    Init,
}

impl SnapshotBuiltin {
    const ALL: [Self; 2] = [Self::Previous, Self::Init];

    fn name(self) -> &'static str {
        match self {
            SnapshotBuiltin::Previous => "PREVIOUS",
            SnapshotBuiltin::Init => "INIT",
        }
    }

    /// The call over `arg`. A `PREVIOUS` takes the fallback `-7`, a value no
    /// fixture below produces otherwise, so the first step is recognizable.
    fn call(self, arg: &str) -> String {
        match self {
            SnapshotBuiltin::Previous => format!("PREVIOUS({arg}, -7)"),
            SnapshotBuiltin::Init => format!("INIT({arg})"),
        }
    }

    fn kind(self) -> CaptureKind {
        match self {
            SnapshotBuiltin::Previous => CaptureKind::Previous,
            SnapshotBuiltin::Init => CaptureKind::Init,
        }
    }

    /// The series a direct read of a constant `selected` produces over the
    /// three saved steps of a `with_sim_time(0, 2, 1)` fixture.
    fn series_of(self, selected: f64) -> Vec<f64> {
        match self {
            SnapshotBuiltin::Previous => vec![-7.0, selected, selected],
            SnapshotBuiltin::Init => vec![selected; 3],
        }
    }
}

/// The captures `var`'s production parse synthesized, by ident, in walk order.
fn capture_idents(db: &SimlinDb, sync: &SyncResult, var: &str) -> Vec<String> {
    parse_source_variable(db, sync.models["main"].variables[var].source, sync.project)
        .implicit_vars
        .iter()
        .filter_map(|helper| helper.capture())
        .map(|capture| capture.ident().to_string())
        .collect()
}

/// Which of the three runlists the helper named `helper` is in, under the
/// no-inputs wiring of `model`.
fn helper_membership(
    db: &SimlinDb,
    sync: &SyncResult,
    model: &str,
    helper: &str,
) -> RunlistMembership {
    implicit_var_runlist_membership(
        db,
        sync.models[model].source,
        sync.project,
        helper.to_string(),
        ModuleInputSet::empty(db),
    )
}

/// The `(initial, flow, stock)` fragment presence of the helper named
/// `helper`, compiled through the production per-helper compiler.
fn helper_fragments(
    db: &SimlinDb,
    sync: &SyncResult,
    model: &str,
    helper: &str,
) -> (bool, bool, bool) {
    let result = compile_implicit_var_fragment(
        db,
        sync.models[model].source,
        sync.project,
        helper.to_string(),
        ModuleInputSet::empty(db),
    )
    .as_ref()
    .unwrap_or_else(|| panic!("{helper} must compile"));
    (
        result.fragment.initial_bytecodes.is_some(),
        result.fragment.flow_bytecodes.is_some(),
        result.fragment.stock_bytecodes.is_some(),
    )
}

/// The number of `PREVIOUS`/`INIT` reads of `of` in `var`'s flow fragment,
/// counted on the symbolic stream so opcode fusion cannot hide one.
fn direct_reads(db: &SimlinDb, sync: &SyncResult, var: &str, of: &str) -> usize {
    use crate::compiler::symbolic::SymbolicOpcode;

    let model = sync.models["main"].source;
    compile_var_fragment(
        db,
        sync.models["main"].variables[var].source,
        model,
        sync.project,
        ModuleInputSet::empty(db),
    )
    .as_ref()
    .unwrap_or_else(|| panic!("{var} must compile"))
    .fragment
    .flow_bytecodes
    .as_ref()
    .unwrap_or_else(|| panic!("{var} must have a flow fragment"))
    .symbolic
    .code
    .iter()
    .filter(|op| match op {
        SymbolicOpcode::SymLoadPrev { var } | SymbolicOpcode::SymLoadInitial { var } => {
            var.name.as_str() == of
        }
        _ => false,
    })
    .count()
}

/// PREVIOUS with a numeric-constant subscript index compiles to a direct
/// LoadPrev (a number is never a variable reference).
#[test]
fn test_previous_numeric_subscript_no_helper() {
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

/// The four direct element arms and the two dynamic controls of a user
/// equation, crossed with both intrinsics, in ONE fixture read through the
/// production parse, dependency graph, layout, fragment compiler and VM.
/// (The same bare element at the generated-LTM parse boundary, where a
/// same-named variable DOES capture, is
/// `db::ltm_tests::a_bare_element_snapshot_captures_on_the_generated_path_only_when_shadowed`.)
///
/// What each surface says: only the runtime-indexed arguments capture; the
/// `INIT` capture is an initials-only helper and the `PREVIOUS` capture a
/// flows-only one (a capture's kind is its phase demand); every capture keeps
/// a layout slot, and the INIT-only one has no results key because nothing
/// writes it per step; each direct arm reads `vals` through exactly one
/// snapshot opcode; and a rerun after `reset` reproduces every value, so the
/// INIT capture is re-populated by initials rather than left over from the
/// prior run.
#[test]
fn user_element_snapshots_are_direct_for_both_intrinsics() {
    let tp = TestProject::new("user_element_snapshots")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("d", &["e1", "e2"])
        .array_with_ranges("vals[d]", vec![("e1", "10 + TIME"), ("e2", "20 + TIME")])
        .aux("idx", "1 + MIN(TIME, 1)", None)
        // XMILE footnote 9: inside `vals[...]` the declared element `e2` hides
        // this same-named variable.
        .aux("e2", "1", None)
        .aux("prev_bare", "PREVIOUS(vals[e2], -1)", None)
        .aux("prev_qualified", "PREVIOUS(vals[d.e1], -2)", None)
        .aux("init_bare", "INIT(vals[e2])", None)
        .aux("init_qualified", "INIT(vals[d.e1])", None)
        .aux("prev_dynamic", "PREVIOUS(vals[idx], -3)", None)
        .aux("init_dynamic", "INIT(vals[idx])", None);

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &tp.build_datamodel());
    let model = sync.models["main"].source;

    for direct in ["prev_bare", "prev_qualified", "init_bare", "init_qualified"] {
        assert_eq!(
            capture_idents(&db, &sync, direct),
            Vec::<String>::new(),
            "{direct}: a static element index needs no capture"
        );
        assert_eq!(
            direct_reads(&db, &sync, direct, "vals"),
            1,
            "{direct}: one snapshot read of `vals` itself"
        );
    }
    let prev_capture = "$\u{205A}prev_dynamic\u{205A}0\u{205A}arg0";
    let init_capture = "$\u{205A}init_dynamic\u{205A}0\u{205A}arg0";
    assert_eq!(capture_idents(&db, &sync, "prev_dynamic"), [prev_capture]);
    assert_eq!(capture_idents(&db, &sync, "init_dynamic"), [init_capture]);
    assert_eq!(direct_reads(&db, &sync, "prev_dynamic", prev_capture), 1);
    assert_eq!(direct_reads(&db, &sync, "init_dynamic", init_capture), 1);

    let graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));
    fn helpers_in(runlist: &[String]) -> Vec<&str> {
        let mut names: Vec<&str> = runlist
            .iter()
            .filter(|name| name.starts_with('$'))
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        names
    }
    assert_eq!(
        helpers_in(&graph.runlist_initials),
        [init_capture],
        "the INIT capture is the only helper in initials"
    );
    assert_eq!(
        helpers_in(&graph.runlist_flows),
        [prev_capture],
        "the PREVIOUS capture is the only helper in flows: an INIT-only capture is \
         populated once and read from the frozen snapshot"
    );

    let layout = compute_layout(&db, model, sync.project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the direct and captured rows compile together");
    for capture in [prev_capture, init_capture] {
        assert!(
            layout.get(capture).is_some(),
            "{capture} keeps its layout slot"
        );
    }
    assert!(
        compiled.get_offset(&Ident::new(prev_capture)).is_some(),
        "a flows-refreshed capture has a series"
    );
    assert!(
        compiled.get_offset(&Ident::new(init_capture)).is_none(),
        "an INIT-only capture has no series: nothing writes its slot per step"
    );

    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    let observe = |vm: &crate::vm::Vm| -> Vec<f64> {
        ["init_bare", "init_qualified", "init_dynamic"]
            .iter()
            .map(|name| vm.get_value_now(vm.get_offset(&Ident::new(name)).expect("offset")))
            .collect()
    };
    vm.run_initials().expect("initials");
    let first = observe(&vm);
    vm.reset();
    vm.run_initials().expect("initials after reset");
    assert_eq!(
        observe(&vm),
        first,
        "a reset re-populates every INIT storage"
    );
    vm.run_to_end().expect("run");
    let values = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(values["prev_bare"], [-1.0, 20.0, 21.0]);
    assert_eq!(values["prev_qualified"], [-2.0, 10.0, 11.0]);
    assert_eq!(values["init_bare"], [20.0, 20.0, 20.0]);
    assert_eq!(values["init_qualified"], [10.0, 10.0, 10.0]);
    assert_eq!(values["prev_dynamic"], [-3.0, 10.0, 21.0]);
    assert_eq!(values["init_dynamic"], [10.0, 10.0, 10.0]);
}

/// Every way an identifier index of a source `PREVIOUS`/`INIT` subscript can
/// relate to the referenced axis, crossed with both intrinsics.
///
/// A row that names one element of the axis reads the slot directly and its
/// VM series is the selected value; a row that names nothing on the axis
/// captures, and the capture's body then fails to lower, loudly, on the
/// variable it was hoisted from. The rows are the enumeration of where a name
/// can come from -- the axis itself, a same-named variable, another dimension
/// (qualified, or bare and ambiguous), a mapped axis on either side of the
/// map, a subdimension -- and nothing else the source rule consults.
#[test]
fn snapshot_element_name_matrix_covers_both_intrinsics() {
    #[derive(Clone, Copy, Debug)]
    enum Case {
        SameNameVariableCollision,
        UnrelatedAxisQualification,
        MissingQualifiedName,
        MissingBareName,
        GloballyAmbiguousBareName,
        MappedAxisOwnElement,
        MappedTargetOnlyElement,
        SubdimensionOwnElement,
    }

    impl Case {
        const ALL: [Case; 8] = [
            Case::SameNameVariableCollision,
            Case::UnrelatedAxisQualification,
            Case::MissingQualifiedName,
            Case::MissingBareName,
            Case::GloballyAmbiguousBareName,
            Case::MappedAxisOwnElement,
            Case::MappedTargetOnlyElement,
            Case::SubdimensionOwnElement,
        ];

        /// The fixture minus the probe, the index spelling, and the value the
        /// index selects -- `None` when it selects nothing on the axis.
        fn fixture(self) -> (TestProject, &'static str, Option<f64>) {
            let base = TestProject::new("snapshot_element_names").with_sim_time(0.0, 2.0, 1.0);
            match self {
                Case::SameNameVariableCollision => (
                    base.named_dimension("Data", &["d1", "d2"])
                        .array_with_ranges("vals[Data]", vec![("d1", "10"), ("d2", "20")])
                        // XMILE footnote 9: hidden only inside `vals[...]`.
                        .aux("d2", "1", None),
                    "d2",
                    Some(20.0),
                ),
                Case::UnrelatedAxisQualification => (
                    base.named_dimension("Data", &["d1", "d2", "d3"])
                        .named_dimension("Selector", &["s1", "s2", "s3"])
                        .array_with_ranges(
                            "vals[Data]",
                            vec![("d1", "10"), ("d2", "20"), ("d3", "30")],
                        ),
                    "Selector.s2",
                    Some(20.0),
                ),
                Case::MissingQualifiedName => (
                    base.named_dimension("Data", &["d1", "d2"])
                        .named_dimension("Selector", &["s1", "s2"])
                        .array_with_ranges("vals[Data]", vec![("d1", "10"), ("d2", "20")]),
                    "Selector.absent",
                    None,
                ),
                Case::MissingBareName => (
                    base.named_dimension("Data", &["d1", "d2"])
                        .array_with_ranges("vals[Data]", vec![("d1", "10"), ("d2", "20")]),
                    "absent",
                    None,
                ),
                Case::GloballyAmbiguousBareName => (
                    base.named_dimension("Data", &["shared", "d2"])
                        .named_dimension("Other", &["o1", "shared"])
                        .array_with_ranges("vals[Data]", vec![("shared", "10"), ("d2", "20")]),
                    "shared",
                    Some(10.0),
                ),
                Case::MappedAxisOwnElement => (
                    base.named_dimension("Target", &["t1", "t2"])
                        .named_dimension_with_mapping("Source", &["s1", "s2"], "Target")
                        .array_with_ranges("vals[Source]", vec![("s1", "10"), ("s2", "20")]),
                    "s2",
                    Some(20.0),
                ),
                Case::MappedTargetOnlyElement => (
                    base.named_dimension("Target", &["t1", "t2"])
                        .named_dimension_with_mapping("Source", &["s1", "s2"], "Target")
                        .array_with_ranges("vals[Source]", vec![("s1", "10"), ("s2", "20")]),
                    "t2",
                    None,
                ),
                Case::SubdimensionOwnElement => (
                    // A named subdimension is one whose elements are a subset
                    // of another's.
                    base.named_dimension("Parent", &["p1", "p2", "p3"])
                        .named_dimension("Sub", &["p2", "p3"])
                        .array_with_ranges("vals[Sub]", vec![("p2", "10"), ("p3", "20")]),
                    "p3",
                    Some(20.0),
                ),
            }
        }
    }

    for case in Case::ALL {
        for builtin in SnapshotBuiltin::ALL {
            let what = format!("{case:?} / {}", builtin.name());
            let (base, index, selected) = case.fixture();
            let tp = base.aux("probe", &builtin.call(&format!("vals[{index}]")), None);
            let project = tp.build_datamodel();
            let db = SimlinDb::default();
            let sync = sync_from_datamodel(&db, &project);

            assert_eq!(
                capture_idents(&db, &sync, "probe").len(),
                usize::from(selected.is_none()),
                "{what}: an index naming one element of the axis needs no capture"
            );

            let Some(selected) = selected else {
                assert!(
                    compile_project_incremental(&db, sync.project, "main").is_err(),
                    "{what}: an index naming nothing on the axis must refuse"
                );
                // The capture's body is what fails to lower: for every
                // refusing row (`MissingQualifiedName`, `MissingBareName`,
                // `MappedTargetOnlyElement`) the helper's fragment refuses,
                // so the diagnostic sits on the helper, which names `probe`;
                // a parse-stage error in the body would sit on `probe`
                // itself, since its span is in the parent's text.
                assert!(
                    tp.error_diagnostics().iter().any(|(location, _)| {
                        location == "main.probe"
                            || location.starts_with("main.$\u{205A}probe\u{205A}")
                    }),
                    "{what}: the refusal must name the variable that wrote the index; \
                     got {:?}",
                    tp.error_diagnostics()
                );
                continue;
            };
            assert_eq!(
                direct_reads(&db, &sync, "probe", "vals"),
                1,
                "{what}: one direct snapshot read of `vals`"
            );
            assert_eq!(
                tp.run_vm().expect("runs")["probe"],
                builtin.series_of(selected),
                "{what}: the selected element's value"
            );
        }
    }
}

/// A bare index that satisfies BOTH classifications -- `Active` is the
/// target's apply-to-all dimension and an element of `vals`' `Selector`
/// axis -- spans first: the argument passes through untouched as an
/// array-shaped read, no capture is minted, and lowering resolves each
/// element's read by its own element-first rule, so every target element
/// reads the `Selector.Active` slot.
///
/// Spans-first is pinned over the classified-index alphabet by
/// `snapshot_arg::tests::the_index_fold_covers_every_combination`; this is the
/// production route to that row, which only the source rule can reach (the
/// generated rule sees no axis and the no-model rule no element).
#[test]
fn an_active_dimension_that_is_also_an_axis_element_spans_first_for_both_intrinsics() {
    for builtin in SnapshotBuiltin::ALL {
        let tp = TestProject::new("snapshot_spans_before_static")
            .with_sim_time(0.0, 2.0, 1.0)
            .named_dimension("Active", &["a1", "a2"])
            .named_dimension("Selector", &["Active", "fixed"])
            .array_with_ranges(
                "vals[Selector]",
                vec![("Active", "10 + TIME"), ("fixed", "20 + TIME")],
            )
            .array_aux("out[Active]", &builtin.call("vals[Active]"));
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &tp.build_datamodel());
        assert_eq!(
            capture_idents(&db, &sync, "out"),
            Vec::<String>::new(),
            "{}: spans-first must not capture",
            builtin.name()
        );
        assert_eq!(
            direct_reads(&db, &sync, "out", "vals"),
            2,
            "{}: one direct read per target element",
            builtin.name()
        );
        let values = tp.run_vm().expect("runs");
        let expected = match builtin {
            SnapshotBuiltin::Previous => [-7.0, 10.0, 11.0],
            SnapshotBuiltin::Init => [10.0, 10.0, 10.0],
        };
        for element in ["a1", "a2"] {
            assert_eq!(
                values[&format!("out[{element}]")],
                expected,
                "{}: element-first value for out[{element}]",
                builtin.name()
            );
        }
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

/// A user capture is a causal node from LTM's point of view, so a static
/// element read that no longer captures changes the score topology in one
/// specific way: the bare and qualified spellings of `a1` name one slot and
/// share ONE arrayed `rate[a1] -> grow` score over `DimA`; `rate[b2]` -- the
/// element winning over the same-named variable `b2`, XMILE footnote 9 --
/// gets its own arrayed score; and only the dynamic `rate[idx]` keeps a
/// capture: ONE structural capture over `DimA` (the body is snapshot-only, so
/// it is captured once, not per element), scored as the arrayed source it is.
///
/// The values say the topology change is representation only: `idx` is `2`,
/// so the dynamic read and `rate[b2]` are the same value with the same
/// per-step change, and each element of the capture's score equals that
/// element of the direct `rate[b2] -> grow` score.
#[test]
fn ltm_snapshot_element_reads_preserve_score_topology_and_values() {
    use salsa::Setter;

    use crate::db::{model_implicit_var_info, model_ltm_variables};
    use crate::test_common::collect_results;

    let project = TestProject::new("ltm_snapshot_element_topology")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "b2"])
        .aux("idx", "2", None)
        .aux("b2", "2", None)
        .array_with_ranges(
            "rate[DimA]",
            vec![("a1", "0.01 + TIME * 0.001"), ("b2", "0.02 + TIME * 0.001")],
        )
        .array_flow(
            "grow[DimA]",
            "pop[DimA] * (PREVIOUS(rate[a1], 0) + PREVIOUS(rate[DimA.a1], 0) + \
             PREVIOUS(rate[idx], 0) + PREVIOUS(rate[b2], 0))",
            None,
        )
        .array_stock("pop[DimA]", "100", &["grow"], &[], None)
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    sync.project.set_ltm_enabled(&mut db).to(true);
    sync.project.set_ltm_discovery_mode(&mut db).to(true);
    let model = sync.models["main"].source;

    let mut helper_names: Vec<String> = model_implicit_var_info(&db, model, sync.project)
        .keys()
        .cloned()
        .collect();
    helper_names.sort();
    assert_eq!(
        helper_names,
        ["$\u{205A}grow\u{205A}0\u{205A}arg0"],
        "only the dynamic read captures, once, structurally over DimA"
    );

    let ltm = model_ltm_variables(&db, model, sync.project);
    let direct_names = [
        "$\u{205A}ltm\u{205A}link_score\u{205A}rate[a1]\u{2192}grow",
        "$\u{205A}ltm\u{205A}link_score\u{205A}rate[b2]\u{2192}grow",
    ];
    for direct_name in direct_names {
        let direct: Vec<&LtmSyntheticVar> = ltm
            .vars
            .iter()
            .filter(|var| var.name == direct_name)
            .collect();
        assert_eq!(direct.len(), 1, "{direct_name}: one coalesced score");
        assert_eq!(direct[0].dimensions, ["DimA"], "{direct_name}: arrayed");
    }
    let capture_scores: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|var| {
            var.name
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}grow\u{205A}")
        })
        .collect();
    assert_eq!(
        capture_scores.len(),
        1,
        "one dynamic call site is one arrayed capture with one arrayed score; got {:?}",
        capture_scores.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    assert_eq!(
        capture_scores[0].dimensions,
        ["DimA"],
        "the capture's score has the capture's shape"
    );

    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let offsets = compiled.offsets.clone();
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let raw = vm.into_results();
    let element_series = |name: &str, slot: usize| -> Vec<f64> {
        let base = offsets[&Ident::new(name)];
        (0..raw.step_count)
            .map(|step| raw.data[step * raw.step_size + base + slot])
            .collect()
    };
    let results = collect_results(&raw);
    // pop = 100 at t0; the four lagged rates sum to 0.06 at t0, 0.064 at t1,
    // 0.068 at t2 (each rises by 0.001 per step and `PREVIOUS` reads the
    // prior step), with the fallback 0 at t0.
    let expected_pop = [100.0, 100.0, 106.0, 112.784];
    let expected_grow = [0.0, 6.0, 6.784, 7.669312000000001];
    for element in ["a1", "b2"] {
        assert_eq!(results[&format!("pop[{element}]")], expected_pop);
        assert_eq!(results[&format!("grow[{element}]")], expected_grow);
    }
    for (slot, element) in ["a1", "b2"].into_iter().enumerate() {
        let direct = element_series(direct_names[1], slot);
        let capture = element_series(&capture_scores[0].name, slot);
        assert_eq!(
            capture, direct,
            "grow[{element}]: the capture's score is the direct read's score"
        );
    }
}

/// A `VECTOR ELM MAP` whose base is a `PREVIOUS` of one bare element reads
/// `vals`' own extent, exactly as the numeric spelling does: the element's
/// slot is the base and the mapping ranges over the whole previous array,
/// so offset 1 from `e1` reaches the prior step's `e2`. A capture -- one
/// slot -- would leave the mapping nothing to reach past the element
/// (`:NA:`, a NaN); this row is the value the direct read gives instead.
#[test]
fn an_elm_map_over_a_bare_element_snapshot_ranges_over_the_variable() {
    let tp = TestProject::new("elm_map_bare_element_snapshot")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("d", &["e1", "e2", "e3"])
        .array_with_ranges(
            "vals[d]",
            vec![
                ("e1", "10 + TIME"),
                ("e2", "20 + TIME"),
                ("e3", "30 + TIME"),
            ],
        )
        .array_aux("offs[d]", "1")
        .array_aux(
            "by_name[d]",
            "VECTOR ELM MAP(PREVIOUS(vals[e1], 0), offs[d])",
        )
        .array_aux(
            "by_number[d]",
            "VECTOR ELM MAP(PREVIOUS(vals[1], 0), offs[d])",
        );
    let values = tp.run_vm().expect("runs");
    for element in ["e1", "e2", "e3"] {
        assert_eq!(
            values[&format!("by_name[{element}]")],
            [0.0, 20.0, 21.0, 22.0],
            "by_name[{element}]: the fallback, then vals[e2] one step back"
        );
        assert_eq!(
            values[&format!("by_name[{element}]")],
            values[&format!("by_number[{element}]")],
            "by_name[{element}]: the bare and numeric spellings are one route"
        );
    }
}

/// Every capture kind crossed with every storage shape the production
/// visitor mints, read through the dependency graph and the per-helper
/// compiler: the runlists a capture is in, and the fragments it has, are its
/// kind. The shapes are the enumeration of `hoist_capture`'s arms and of
/// what a body can hold -- a computed scalar, a runtime-selected element, an
/// array-valued nested snapshot (a structural apply-to-all capture), and a body
/// whose only dependency is an `INIT` read (the "fully determined at
/// initialization" seed signature, which must not seed a `PREVIOUS` capture).
#[test]
fn every_capture_kind_has_the_right_phases_for_every_storage_shape() {
    #[derive(Clone, Copy, Debug)]
    enum Shape {
        ComputedScalar,
        DynamicElement,
        ArrayValued,
        InitialBacked,
    }

    impl Shape {
        const ALL: [Shape; 4] = [
            Shape::ComputedScalar,
            Shape::DynamicElement,
            Shape::ArrayValued,
            Shape::InitialBacked,
        ];

        fn argument(self) -> &'static str {
            match self {
                Shape::ComputedScalar => "k * 2",
                Shape::DynamicElement => "vals[idx]",
                Shape::ArrayValued => "PREVIOUS(vals)",
                Shape::InitialBacked => "INIT(k) + 1",
            }
        }
    }

    for builtin in SnapshotBuiltin::ALL {
        for shape in Shape::ALL {
            let what = format!("{}/{shape:?}", builtin.name());
            let equation = builtin.call(shape.argument());
            let base = TestProject::new("capture_phase_matrix")
                .with_sim_time(0.0, 2.0, 1.0)
                .named_dimension("d", &["e1", "e2"])
                .aux("k", "3", None)
                .aux("idx", "1 + MIN(TIME, 1)", None)
                .array_with_ranges("vals[d]", vec![("e1", "10"), ("e2", "20")]);
            let tp = match shape {
                Shape::ArrayValued => base.array_aux("target[d]", &equation),
                _ => base.aux("target", &equation, None),
            };
            let db = SimlinDb::default();
            let sync = sync_from_datamodel(&db, &tp.build_datamodel());
            let captures: Vec<_> = parse_source_variable(
                &db,
                sync.models["main"].variables["target"].source,
                sync.project,
            )
            .implicit_vars
            .iter()
            .filter_map(|helper| helper.capture())
            .cloned()
            .collect();
            assert_eq!(captures.len(), 1, "{what}: one outer capture");
            assert_eq!(captures[0].kind(), builtin.kind(), "{what}");
            let name = captures[0].ident();

            let kind = builtin.kind();
            assert_eq!(
                helper_membership(&db, &sync, "main", name),
                RunlistMembership {
                    initials: kind.needs_initials(),
                    flows: kind.needs_flows(),
                    stocks: false,
                },
                "{what}: runlist membership is the kind's phase demand"
            );
            assert_eq!(
                helper_fragments(&db, &sync, "main", name),
                (kind.needs_initials(), kind.needs_flows(), false),
                "{what}: fragment presence is the kind's phase demand"
            );
            tp.assert_compiles_incremental();
        }
    }
}

/// A `PREVIOUS` capture whose body reads `INIT(k)` has the seed signature of
/// an initialization-backed variable (no dt dependency, an initial one) and
/// is NOT seeded into initials: nothing reads its initials value. Its `INIT`
/// referent is an initialization root of its own, so `k` is frozen before
/// the first flow evaluation reads it.
#[test]
fn a_previous_capture_flow_seeds_its_local_init_referent() {
    let tp = TestProject::new("previous_capture_local_init_referent")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("k", "3 + TIME", None)
        .aux("out", "PREVIOUS(INIT(k) + 1, -7)", None);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &tp.build_datamodel());
    let graph = model_dependency_graph(
        &db,
        sync.models["main"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert!(graph.runlist_initials.iter().any(|name| name == "k"));
    assert_eq!(
        helper_membership(&db, &sync, "main", "$\u{205A}out\u{205A}0\u{205A}arg0"),
        RunlistMembership {
            initials: false,
            flows: true,
            stocks: false,
        }
    );
    assert_eq!(tp.run_vm().expect("runs")["out"], [-7.0, 4.0, 4.0, 4.0]);
}

/// The dt and active-initial passes restart the walk counter, so when they
/// mint the same body for different snapshot consumers the two are one
/// capture whose phase demand is the union, in either parser order.
///
/// `ACTIVE INITIAL` has no `TestProject` spelling; it is an importer-only
/// field (`compat.active_initial`) set on the datamodel here, exactly as the
/// MDL reader sets it.
#[test]
fn a_positional_capture_shared_by_init_and_previous_unions_its_phases() {
    struct Row {
        dt_equation: &'static str,
        active_initial: &'static str,
        initial_out: f64,
        expected_out: [f64; 3],
        expected_observed: [f64; 3],
    }

    let rows = [
        Row {
            dt_equation: "INIT(k * 2)",
            active_initial: "PREVIOUS(k * 2, -7)",
            initial_out: -7.0,
            expected_out: [2.0, 2.0, 2.0],
            expected_observed: [-7.0, -7.0, -7.0],
        },
        Row {
            dt_equation: "PREVIOUS(k * 2, -7)",
            active_initial: "INIT(k * 2)",
            initial_out: 2.0,
            expected_out: [-7.0, 2.0, 4.0],
            expected_observed: [2.0, 2.0, 2.0],
        },
    ];

    for row in rows {
        let mut project = TestProject::new("capture_phase_union")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux("k", "1 + TIME", None)
            .aux("out", row.dt_equation, None)
            .aux("observed_initial", "INIT(out)", None)
            .build_datamodel();
        let datamodel::Variable::Aux(out) = project.models[0]
            .variables
            .iter_mut()
            .find(|variable| variable.get_ident() == "out")
            .expect("out")
        else {
            unreachable!("the fixture builds out as an aux")
        };
        out.compat.active_initial = Some(row.active_initial.to_string());

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let parsed = parse_source_variable(
            &db,
            sync.models["main"].variables["out"].source,
            sync.project,
        );
        assert!(
            parsed.variable.errors.is_empty(),
            "{}: one storage read by two consumers is not a helper collision",
            row.dt_equation
        );
        let captures: Vec<_> = parsed
            .implicit_vars
            .iter()
            .filter_map(|helper| helper.capture())
            .collect();
        assert_eq!(captures.len(), 1, "{}: one shared capture", row.dt_equation);
        assert_eq!(captures[0].kind(), CaptureKind::PreviousAndInit);
        let name = captures[0].ident();
        assert_eq!(
            helper_membership(&db, &sync, "main", name),
            RunlistMembership {
                initials: true,
                flows: true,
                stocks: false,
            }
        );
        assert_eq!(
            helper_fragments(&db, &sync, "main", name),
            (true, true, false)
        );

        let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
        let mut vm = crate::vm::Vm::new(compiled).expect("vm");
        let out_offset = vm.get_offset(&Ident::new("out")).expect("out");
        let observed_offset = vm
            .get_offset(&Ident::new("observed_initial"))
            .expect("observed");
        vm.run_initials().expect("initials");
        assert_eq!(vm.get_value_now(out_offset), row.initial_out);
        assert_eq!(vm.get_value_now(observed_offset), row.initial_out);
        vm.reset();
        vm.run_initials().expect("initials after reset");
        assert_eq!(vm.get_value_now(out_offset), row.initial_out);
        vm.run_to_end().expect("runs");
        let values = crate::test_common::collect_results(&vm.into_results());
        assert_eq!(values["out"], row.expected_out);
        assert_eq!(values["observed_initial"], row.expected_observed);
    }
}

/// `INIT` is only the default consumer of its capture: a per-step definition
/// that reads the helper's CURRENT value -- here by its name, which the
/// lexer accepts quoted -- promotes it into flows, so the value it sees is
/// the capture's live one rather than the frozen snapshot. The promoted
/// capture is still hidden from the results map: hiding is decided by its
/// kind, and a written slot nobody exposes costs nothing.
#[test]
fn a_current_value_consumer_promotes_an_init_capture_into_flows() {
    let helper = "$\u{205A}frozen\u{205A}0\u{205A}arg0";
    let tp = TestProject::new("init_capture_current_consumer")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("k", "1 + TIME", None)
        .aux("frozen", "INIT(k * 2)", None)
        .aux("observer", &format!("\"{helper}\""), None);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &tp.build_datamodel());
    assert_eq!(
        helper_membership(&db, &sync, "main", helper),
        RunlistMembership {
            initials: true,
            flows: true,
            stocks: false,
        }
    );
    assert_eq!(
        helper_fragments(&db, &sync, "main", helper),
        (true, true, false)
    );
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    assert!(compiled.get_offset(&Ident::new(helper)).is_none());

    let values = tp.run_vm().expect("runs");
    assert_eq!(values["frozen"], [2.0, 2.0, 2.0]);
    assert_eq!(values["observer"], [2.0, 4.0, 6.0]);
}

/// A current-value read from ANOTHER INIT-only capture promotes nothing:
/// both are initials work, and deriving the promotion set from every graph
/// node rather than from the definitions that run in flows by kind would
/// refresh the inner helper every step for no reader.
#[test]
fn an_init_only_capture_dependency_does_not_promote_its_input() {
    let inner = "$\u{205A}inner\u{205A}0\u{205A}arg0";
    let outer = "$\u{205A}outer\u{205A}0\u{205A}arg0";
    let tp = TestProject::new("init_capture_chain")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("k", "1 + TIME", None)
        .aux("inner", "INIT(k * 2)", None)
        .aux("outer", &format!("INIT(\"{inner}\" + 1)"), None);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &tp.build_datamodel());
    for helper in [inner, outer] {
        assert_eq!(
            helper_membership(&db, &sync, "main", helper),
            RunlistMembership {
                initials: true,
                flows: false,
                stocks: false,
            },
            "{helper} is reached from INIT consumers only"
        );
        assert_eq!(
            helper_fragments(&db, &sync, "main", helper),
            (true, false, false)
        );
    }
    let values = tp.run_vm().expect("runs");
    assert_eq!(values["inner"], [2.0, 2.0, 2.0]);
    assert_eq!(values["outer"], [3.0, 3.0, 3.0]);
}

/// A computed `INIT` argument inside a bound module is a capture of the
/// sub-model, scheduled under the instance's input set: initials only, and
/// a repeated root initialization reads the same bound value.
#[test]
fn a_bound_module_init_capture_is_initials_only() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

    let mut input = x_aux("input", "0", None);
    let datamodel::Variable::Aux(input_aux) = &mut input else {
        unreachable!("x_aux constructs an aux")
    };
    input_aux.compat.can_be_module_input = true;
    let child = x_model("child", vec![input, x_aux("out", "INIT(input * 2)", None)]);
    let main = x_model(
        "main",
        vec![
            x_aux("source", "3 + TIME", None),
            x_module("child", &[("source", "child.input")], None),
            x_aux("observed", "INIT(child.out)", None),
        ],
    );
    let mut specs = sim_specs_with_units("month");
    specs.stop = 2.0;
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &x_project(specs, &[main, child]));
    let child_model = sync.models["child"].source;
    let helper = "$\u{205A}out\u{205A}0\u{205A}arg0";
    assert_eq!(
        parse_source_variable(
            &db,
            sync.models["child"].variables["out"].source,
            sync.project
        )
        .implicit_vars
        .iter()
        .filter_map(|h| h.capture())
        .map(|c| (c.ident().to_string(), c.kind()))
        .collect::<Vec<_>>(),
        [(helper.to_string(), CaptureKind::Init)]
    );

    let input_sets = crate::db::assemble::module_input_sets_for(&db, sync.project, "main", "child");
    assert_eq!(input_sets.len(), 1, "one child instance");
    let inputs = ModuleInputSet::from_canonical_set(&db, &input_sets[0]);
    assert_eq!(
        implicit_var_runlist_membership(&db, child_model, sync.project, helper.to_string(), inputs),
        RunlistMembership {
            initials: true,
            flows: false,
            stocks: false,
        }
    );
    let fragment =
        compile_implicit_var_fragment(&db, child_model, sync.project, helper.to_string(), inputs)
            .as_ref()
            .expect("the bound capture compiles");
    assert!(fragment.fragment.initial_bytecodes.is_some());
    assert!(fragment.fragment.flow_bytecodes.is_none());

    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    assert!(
        compiled
            .get_offset(&Ident::new(
                "child\u{00B7}$\u{205A}out\u{205A}0\u{205A}arg0"
            ))
            .is_none(),
        "the sub-model's INIT-only capture has no series either"
    );
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    let observed = vm.get_offset(&Ident::new("observed")).expect("observed");
    vm.run_initials().expect("initials");
    assert_eq!(vm.get_value_now(observed), 6.0);
    vm.reset();
    vm.run_initials().expect("initials after reset");
    assert_eq!(vm.get_value_now(observed), 6.0);
    vm.run_to_end().expect("runs");
    assert_eq!(
        crate::test_common::collect_results(&vm.into_results())["observed"],
        [6.0, 6.0, 6.0]
    );
}

use crate::snapshot_arg::SnapshotAccess;

/// One `PREVIOUS`/`INIT` argument shape, and what each of the two decisions
/// `snapshot_arg::SnapshotArg::access` states for it.
struct AgreementRow {
    /// Which arm of the parse's old rule and of codegen's old rule this row
    /// covers, so the table can be read back against both enumerations.
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
/// Rows are derived from BOTH of the rule sets that statement replaced:
///
/// * the parse's (`BuiltinVisitor::snapshot_arg` over the source argument) --
///   a `Var` whose base is or is not module-backed, a `Subscript` whose every
///   index is static / leaves a dimension standing (one of the parent's axes,
///   or a dimension one of them relates to) / is dynamic, and the catch-all
///   for anything that is not a reference;
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
/// * A base that is a module instance, a module output port or a bound
///   input port. The parse leaves every such reference in place (it reads
///   nothing of the owning model) and lowering decides what the snapshot
///   addresses from the dependency's shape; those arms need an explicit
///   sub-model, which `TestProject` does not build, and are the rows of
///   `module_snapshot_arguments_are_resolved_at_lowering` below.
/// * A module instance synthesized in the SAME walk (`PREVIOUS(SMTH1(k, 2))`),
///   the one module-backed base the parse does know: captured, and pinned by
///   `db::capture_tests`.
/// * An index that is BOTH `SpansDimension` and `Static` -- a name that is an
///   active apply-to-all dimension and also an element of the referenced
///   axis. Spans-first is stated once in `SnapshotArg::subscripted` and
///   pinned over the classified-index alphabet by its own test there; the
///   production route to it, with values, is
///   `an_active_dimension_that_is_also_an_axis_element_spans_first_for_both_intrinsics`.
/// * Codegen's `Expr::TempArray` and residual refusals. They are unreachable
///   because the parse captures every non-storage argument first -- which is
///   what the capture rows below establish, by showing the helper's own
///   lowered body is the shape codegen would otherwise have seen.
#[test]
fn every_prev_init_argument_shape_agrees_between_the_parse_and_codegen() {
    use crate::compiler::fragment::lower_fragment;
    use crate::compiler::{BuiltinFn, Expr, lowered_snapshot_arg};
    use crate::db::fragment_compile::implicit_fragment_input;

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
            covers: "visitor: Var naming a stdlib-call aux. codegen: Expr::Var",
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
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
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
            covers: "visitor: Subscript, bare element of the referenced axis. \
                     codegen: collapsed StaticSubscript",
            a2a: false,
            equation: "PREVIOUS(vals[e1], 0)",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
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
            covers: "visitor: Subscript naming a FOREIGN dimension the parent's axis relates \
                     to through a declared mapping (`other` maps to `d`), the compiler's \
                     `active_dim_ref` matcher under `DirectMappingsOnly`. codegen: collapsed \
                     StaticSubscript",
            a2a: true,
            equation: "PREVIOUS(vals_o[other], 0)",
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
            covers: "INIT twin of the scalar qualified-element row",
            a2a: false,
            equation: "INIT(vals[d.e2])",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "INIT twin of the bare-element row",
            a2a: false,
            equation: "INIT(vals[e1])",
            captures: false,
            codegen: Some(SnapshotAccess::Slot),
            divergence: None,
        },
        AgreementRow {
            covers: "INIT twin of the dynamic-index row",
            a2a: false,
            equation: "INIT(vals[idx])",
            captures: true,
            codegen: Some(SnapshotAccess::Capture),
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
                .named_dimension_with_mapping("other", &["o1", "o2", "o3"], "d")
                .array_with_ranges("vals[d]", vec![("e1", "30"), ("e2", "10"), ("e3", "20")])
                .array_with_ranges("vals_o[other]", vec![("o1", "3"), ("o2", "1"), ("o3", "2")])
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

/// One parent and one sub-model, for the snapshot arguments only a project
/// with a module can spell.
///
/// `main`: `src = TIME + 3`, `sub` instantiating `producer` with `src ->
/// sub.input`, and `probe` holding the equation under test. `producer`:
/// `input` (an `access="input"` port), `arr[d] = input * d` over `d = {1, 2,
/// 3}`, `output = input * 10`, and the two snapshot reads of the bound port,
/// `lagged_input = PREVIOUS(input, 0)` and `frozen_input = INIT(input)`.
fn module_snapshot_project(probe_equation: &str) -> datamodel::Project {
    let aux = |ident: &str, equation: datamodel::Equation, can_be_module_input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input,
                ..datamodel::Compat::default()
            },
        })
    };
    let scalar = |ident: &str, eqn: &str, port: bool| {
        aux(ident, datamodel::Equation::Scalar(eqn.to_string()), port)
    };
    let mut project = TestProject::new("module_snapshots")
        .with_sim_time(0.0, 3.0, 1.0)
        .indexed_dimension("d", 3)
        .aux("src", "TIME + 3", None)
        .aux("probe", probe_equation, None)
        .build_datamodel();
    project.models.push(datamodel::Model {
        name: "producer".to_string(),
        sim_specs: None,
        variables: vec![
            scalar("input", "0", true),
            aux(
                "arr",
                datamodel::Equation::ApplyToAll(vec!["d".to_string()], "input * d".to_string()),
                false,
            ),
            scalar("output", "input * 10", false),
            scalar("lagged_input", "PREVIOUS(input, 0)", false),
            scalar("frozen_input", "INIT(input)", false),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "sub".to_string(),
            model_name: "producer".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "src".to_string(),
                dst: "sub.input".to_string(),
            }],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    project
}

/// Every variable's series, step-major, keyed by canonical name (`module·var`
/// for a sub-model's).
fn module_snapshot_series(
    db: &SimlinDb,
    project: SourceProject,
) -> std::collections::HashMap<String, Vec<f64>> {
    let compiled = compile_project_incremental(db, project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = vm.into_results();
    results
        .offsets
        .iter()
        .map(|(name, off)| {
            let series = (0..results.step_count)
                .map(|step| results.data[step * results.step_size + off])
                .collect();
            (name.as_str().to_string(), series)
        })
        .collect()
}

/// The snapshot arguments whose meaning depends on what the NAME denotes in
/// the owning model -- a module instance, one of its output ports, a bound
/// input port -- are resolved at lowering (`Context::snapshot_storage`), not
/// captured by the parse, which reads nothing of the owning model.
///
/// The rows are the enumeration of what a bare name can denote there,
/// crossed with the two intrinsics:
///
/// * a scalar output port and an element of an arrayed one: a fixed slot
///   inside the instance, read directly;
/// * a bound input port, read from inside the sub-model: its own slot, which
///   its fragment assigns the parent's value every phase, so the lag and the
///   freeze are exactly what a capture of the port would have held (the same
///   port wired WITHOUT `access="input"`, and the helper list, are
///   `fragment_determinism_tests::a_submodel_reading_a_bound_port_through_previous_parses_once_and_compiles`);
/// * the bare instance: no storage of its own, refused loudly rather than
///   read at whichever sub-model variable the layout put first.
///
/// Values, from the rules: `src` is `3, 4, 5, 6`, so `sub·output` is `30, 40,
/// 50, 60` and `sub·arr[2]` is `6, 8, 10, 12`; a `PREVIOUS` is its fallback at
/// t=0 and the prior step's value after, an `INIT` the t=0 value throughout.
/// The `INIT` rows read the sub-model's t=0 value because every value-bearing
/// variable of an instantiated model is an initials member (GH #1028).
#[test]
fn module_snapshot_arguments_are_resolved_at_lowering() {
    let rows: &[(&str, Vec<f64>)] = &[
        ("PREVIOUS(sub.output, 0)", vec![0.0, 30.0, 40.0, 50.0]),
        ("INIT(sub.output)", vec![30.0, 30.0, 30.0, 30.0]),
        ("PREVIOUS(sub.arr[2], 0)", vec![0.0, 6.0, 8.0, 10.0]),
        ("INIT(sub.arr[2])", vec![6.0, 6.0, 6.0, 6.0]),
    ];
    for (equation, expected) in rows {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &module_snapshot_project(equation));
        let main = sync.models["main"].source;
        let producer = sync.models["producer"].source;
        assert!(
            model_implicit_var_info(&db, main, sync.project).is_empty()
                && model_implicit_var_info(&db, producer, sync.project).is_empty(),
            "`{equation}`: neither the parent's read of a port nor the sub-model's \
             reads of its bound input synthesize a capture"
        );
        let series = module_snapshot_series(&db, sync.project);
        assert_eq!(&series["probe"], expected, "`{equation}`");
        assert_eq!(
            series["sub\u{00B7}lagged_input"],
            vec![0.0, 3.0, 4.0, 5.0],
            "`PREVIOUS(input, 0)` of the bound port lags the parent's value"
        );
        assert_eq!(
            series["sub\u{00B7}frozen_input"],
            vec![3.0, 3.0, 3.0, 3.0],
            "`INIT(input)` of the bound port freezes the parent's t=0 value"
        );
    }

    for equation in ["PREVIOUS(sub, 0)", "INIT(sub)"] {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &module_snapshot_project(equation));
        let refusals: Vec<String> = collect_all_diagnostics(&db, sync.project)
            .into_iter()
            .filter(|d| d.model == "main" && d.variable.as_deref() == Some("probe"))
            .filter_map(|d| match d.error {
                DiagnosticError::Equation(err)
                    if err.code == crate::common::ErrorCode::NotSimulatable =>
                {
                    err.details
                }
                _ => None,
            })
            .collect();
        assert!(
            refusals
                .iter()
                .any(|why| why.contains("bare module instance 'sub'")),
            "`{equation}`: a bare module instance has no snapshot storage and must be \
             refused on `probe`; got {refusals:?}"
        );
    }
}
