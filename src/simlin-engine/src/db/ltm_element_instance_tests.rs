// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An implicit instance minted by per-element expansion belongs to ONE element.
//!
//! When a variable's equation calls a module function (a stdlib `SMTH1`/`DELAY1`,
//! or a project macro), `builtins_visitor::instantiate_implicit_modules` expands
//! the equation per element and mints a fresh module instance -- plus its
//! argument-capture helper auxes -- for each one. Those synthetic nodes are
//! SCALAR: `$⁚growth⁚0⁚smth1⁚north` is the instance belonging to `growth[north]`
//! and to no other slot.
//!
//! Nothing in the LTM layer used to know that, and two consumers paid for it:
//!
//! * the element causal graph broadcast a scalar instance across every element
//!   of its arrayed parent (and every element of an arrayed source into every
//!   per-element helper), manufacturing cross-element circuits that do not
//!   exist -- the same class of phantom edge `RefShape::PerElement` was
//!   introduced to kill (GH #525);
//! * the module link-score path built one partial for the whole arrayed target
//!   and `scalarize`d it to the FIRST element's arm, so every instance but the
//!   first scored the wrong element, and every arrayed dependency was left as a
//!   bare whole-array `PREVIOUS` that codegen rejects -- a constant-0 score.
//!
//! These tests pin the contract for both. The load-bearing one is
//! [`qualified_index_edge_is_positional_not_by_name`]: the per-element expansion
//! spells its captured subscript as the QUALIFIED `dim·element` form, which the
//! simulation resolves POSITIONALLY (`compiler::subscript` lowers the constified
//! index to `IndexOp::Single(value - 1)`, a raw offset into the subscripted
//! variable's own axis). A describer that resolves it by NAME names rows the
//! simulation never reads -- so that test uses two dimensions carrying the same
//! element names in opposite orders, where the two readings disagree, and checks
//! the graph against the VM rather than against an assumption.
//!
//! ARMS NOT COVERED HERE, deliberately. The walker records a module-output
//! reference at three sites, and these fixtures exercise one: the bare
//! `Expr2::Var` arm (`SMTH1(...) * 0.1` reads `module·port` bare). The
//! `Expr2::Subscript` arm needs an arrayed module OUTPUT subscripted at the
//! reference site (`mod·out[Region]`), and the `BuiltinContents::Ident` arm
//! needs a module output in a builtin's ident position; neither is reachable
//! from a stdlib `SMTH1`/`DELAY1` expansion, which is the shape every arrayed
//! module call in `test/` takes and the shape this contract was written for.
//! All three record the same `RefShape::Bare` with the same `target_element` --
//! three lines of one statement, not three rules -- so deleting either of the
//! other two is currently caught by nothing. That is a disclosed coverage hole,
//! not a claim that those arms do not matter: closing it needs a user sub-model
//! fixture with an arrayed output.

use crate::db::{
    DiagnosticError, SimlinDb, collect_all_diagnostics, model_edge_shapes,
    model_element_causal_edges, model_ltm_variables, set_project_ltm_enabled, sync_from_datamodel,
    sync_from_datamodel_incremental,
};
use crate::test_common::TestProject;

/// The element-level causal edges of the fixture's `main` model.
fn element_edges(project: &TestProject) -> super::ElementCausalEdgesResult {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    model_element_causal_edges(&db, sync.models["main"].source, sync.project).clone()
}

fn assert_edge(result: &super::ElementCausalEdgesResult, from: &str, to: &str) {
    let targets = result.edges.get(from);
    assert!(
        targets.is_some_and(|ts| ts.contains(to)),
        "expected edge {from} -> {to}, but it was missing.\nedges from '{from}': {targets:?}"
    );
}

fn assert_no_edge(result: &super::ElementCausalEdgesResult, from: &str, to: &str) {
    let has = result.edges.get(from).is_some_and(|ts| ts.contains(to));
    assert!(
        !has,
        "expected NO edge {from} -> {to}, but it was present.\nedges from '{from}': {:?}",
        result.edges.get(from)
    );
}

/// An arrayed stock whose inflow smooths the stock: the loop
/// `stock[e] -> arg0[e] -> smth1[e] -> growth[e] -> stock[e]`, once per element.
/// This is C-LEARN's `Emissions with Stopped Growth[COP]` shape reduced to two
/// elements -- an `Equation::ApplyToAll` body containing a module call.
/// The run is long enough for the loop to actually turn: the score's guard form
/// yields 0 at `INITIAL_TIME`, and the smoothing stock needs several steps
/// before its output moves, so a default 2-step run reads all zeros whether or
/// not the fragment compiled.
fn smooth_loop_fixture() -> TestProject {
    TestProject::new("arrayed_smooth_loop")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("Region", &["north", "south"])
        .array_stock("stock[Region]", "10", &["growth"], &[], None)
        .array_flow("growth[Region]", "SMTH1(stock[Region], 1) * 0.1", None)
}

// ---------------------------------------------------------------------------
// The element graph
// ---------------------------------------------------------------------------

/// A module instance feeds ONLY the element it was minted for.
///
/// `growth`'s lowered AST is `Arrayed(["region"], {north:
/// "$⁚growth⁚0⁚smth1⁚north·output" * 0.1, south: ...})` -- the north slot
/// references the north instance and nothing else -- so the broadcast to
/// `growth[south]` has no reference behind it at all.
#[test]
fn module_instance_feeds_only_its_own_element() {
    let edges = element_edges(&smooth_loop_fixture());

    assert_edge(&edges, "$⁚growth⁚0⁚smth1⁚north", "growth[north]");
    assert_no_edge(&edges, "$⁚growth⁚0⁚smth1⁚north", "growth[south]");

    assert_edge(&edges, "$⁚growth⁚0⁚smth1⁚south", "growth[south]");
    assert_no_edge(&edges, "$⁚growth⁚0⁚smth1⁚south", "growth[north]");
}

/// The mirror direction: an arrayed source feeds ONLY the per-element helper
/// that captured that element.
///
/// `$⁚growth⁚0⁚arg0⁚north`'s equation is `stock[region·north]`. `Region` indexes
/// `stock`'s own dimension here, so position and name agree and the true edge is
/// the diagonal; [`qualified_index_edge_is_positional_not_by_name`] is the case
/// that separates the two readings.
#[test]
fn arrayed_source_feeds_only_the_helper_that_captured_it() {
    let edges = element_edges(&smooth_loop_fixture());

    assert_edge(&edges, "stock[north]", "$⁚growth⁚0⁚arg0⁚north");
    assert_no_edge(&edges, "stock[north]", "$⁚growth⁚0⁚arg0⁚south");

    assert_edge(&edges, "stock[south]", "$⁚growth⁚0⁚arg0⁚south");
    assert_no_edge(&edges, "stock[south]", "$⁚growth⁚0⁚arg0⁚north");
}

/// THE test that separates a positional reading from a name-based one.
///
/// `Other` declares the same two element NAMES as `Region` in the opposite
/// ORDER. The per-element expansion of `out[Region]` captures
/// `stock[region·north]`, and `compiler::subscript` resolves that constified
/// index as a raw position into `stock`'s own axis (`Other`), so it reads
/// `Other`'s FIRST element -- `stock[south]`. The VM assertion below is the
/// oracle: it fixes which element is read without appealing to the graph, so a
/// name-based "fix" to the element graph fails this test rather than passing it.
#[test]
fn qualified_index_edge_is_positional_not_by_name() {
    let project = TestProject::new("qualified_positional")
        .named_dimension("Region", &["north", "south"])
        .named_dimension("Other", &["south", "north"])
        .array_with_ranges_direct(
            "stock",
            vec!["Other".to_string()],
            vec![("south", "10"), ("north", "20")],
            None,
        )
        .array_aux("out[Region]", "SMTH1(stock[Region], 1)");

    // Oracle first: what does the simulation actually read?
    let run = project.run_vm_expecting_success();
    let north_helper = run
        .get("$⁚out⁚0⁚arg0⁚north")
        .expect("the north slot's capture helper should exist");
    assert_eq!(
        north_helper.last().copied(),
        Some(10.0),
        "`stock[region·north]` must read stock's FIRST positional element \
         (south == 10), not the element NAMED north (20)"
    );

    // The graph must describe that same read. The paired positive assertion on
    // `stock[north]` is what keeps the negative one from passing vacuously if a
    // node-naming change ever made `stock[north]` an unknown key.
    let edges = element_edges(&project);
    assert_edge(&edges, "stock[south]", "$⁚out⁚0⁚arg0⁚north");
    assert_no_edge(&edges, "stock[north]", "$⁚out⁚0⁚arg0⁚north");
    assert_edge(&edges, "stock[north]", "$⁚out⁚0⁚arg0⁚south");
    assert_no_edge(&edges, "stock[south]", "$⁚out⁚0⁚arg0⁚south");
}

/// The phantom circuits are gone: two independent per-element loops, not the
/// cross-product of them.
#[test]
fn per_element_module_loops_do_not_cross_elements() {
    let datamodel = smooth_loop_fixture().build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    #[allow(deprecated)]
    let circuits =
        super::model_element_loop_circuits(&db, sync.models["main"].source, sync.project).clone();

    assert_eq!(
        circuits.circuits.len(),
        2,
        "expected exactly one loop per element; got {} circuits:\n{:#?}",
        circuits.circuits.len(),
        circuits.circuits
    );
    for circuit in &circuits.circuits {
        let nodes: Vec<&str> = circuit
            .iter()
            .map(|i| circuits.names[*i as usize].as_str())
            .collect();
        let touches_north = nodes.iter().any(|n| n.contains("north"));
        let touches_south = nodes.iter().any(|n| n.contains("south"));
        assert!(
            touches_north ^ touches_south,
            "a per-element module loop must stay within one element, got {nodes:?}"
        );
    }
}

/// Control: a SCALAR parent's instance is not element-bound, and the
/// per-element narrowing must leave it exactly as it is.
///
/// Note the scalar fixture synthesizes NO capture helper: `SMTH1(stock, 1)`
/// passes a bare `Var`, so `hoist_capture` is never reached and the stock wires
/// straight into the instance. (In the arrayed fixture the same call becomes
/// `SMTH1(stock[region·north], 1)` -- a `Subscript`, which IS hoisted.) This
/// test passes at HEAD; it is here to fail if the fix over-reaches.
#[test]
fn scalar_module_instance_keeps_its_plain_edges() {
    let project = TestProject::new("scalar_smooth_loop")
        .stock("stock", "10", &["growth"], &[], None)
        .flow("growth", "SMTH1(stock, 1) * 0.1", None);
    let edges = element_edges(&project);

    assert_edge(&edges, "stock", "$⁚growth⁚0⁚smth1");
    assert_edge(&edges, "$⁚growth⁚0⁚smth1", "growth");
}

// ---------------------------------------------------------------------------
// The link scores
// ---------------------------------------------------------------------------

/// Every LTM synthetic variable the fixture emits must compile.
///
/// A failing fragment keeps its layout slot with no bytecode and reads a
/// constant 0, so this is the difference between "no score" and "a score that
/// silently is not one".
#[test]
fn arrayed_module_loop_emits_no_failing_fragments() {
    let datamodel = smooth_loop_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let failures: Vec<String> = collect_all_diagnostics(&db, sync.project)
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Assembly(msg) if msg.contains("failed to compile") => {
                Some(format!("{:?}: {msg}", d.variable))
            }
            _ => None,
        })
        .collect();

    assert!(
        failures.is_empty(),
        "expected every LTM fragment to compile, but {} failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The module link score is emitted PER TARGET ELEMENT, and each instance's
/// score holds ITS OWN output live.
///
/// Before the fix a single scalar `…smth1⁚north→growth` carried element 0's arm
/// for every instance, so the south instance's score referenced the north
/// instance -- frozen at `PREVIOUS`, making the numerator entirely lagged. That
/// is a wrong answer masked by a compile failure, not merely a missing one.
#[test]
fn module_link_scores_are_per_target_element_and_hold_their_own_source_live() {
    let datamodel = smooth_loop_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    for elem in ["north", "south"] {
        let name = format!("$⁚ltm⁚link_score⁚$⁚growth⁚0⁚smth1⁚{elem}→growth[{elem}]");
        let var = ltm.vars.iter().find(|v| v.name == name).unwrap_or_else(|| {
            let emitted: Vec<&str> = ltm
                .vars
                .iter()
                .map(|v| v.name.as_str())
                .filter(|n| n.contains("smth1"))
                .collect();
            panic!("expected a per-target-element module link score {name}; emitted: {emitted:#?}")
        });

        let text = var.equation.source_text();
        let own = format!("$⁚growth⁚0⁚smth1⁚{elem}·output");
        assert!(
            text.contains(&own),
            "{name} must reference its own instance's output; equation:\n{text}"
        );
        let other = if elem == "north" { "south" } else { "north" };
        let foreign = format!("$⁚growth⁚0⁚smth1⁚{other}·output");
        assert!(
            !text.contains(&foreign),
            "{name} must not reference the {other} instance at all; equation:\n{text}"
        );
    }
}

/// End to end: both per-element loops score, and neither is a constant zero.
#[test]
fn both_per_element_module_loops_score_nonzero() {
    let project = smooth_loop_fixture();
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let loop_scores: Vec<String> = ltm
        .vars
        .iter()
        .map(|v| v.name.clone())
        .filter(|n| n.starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"))
        .collect();
    assert_eq!(
        loop_scores.len(),
        2,
        "expected one loop score per element loop, got {loop_scores:?}"
    );

    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    for name in &loop_scores {
        let offset = *compiled
            .offsets
            .get(name.as_str())
            .unwrap_or_else(|| panic!("{name} has no results offset"));
        let series: Vec<f64> = (0..results.step_count)
            .map(|step| results.data[step * results.step_size + offset])
            .collect();
        assert!(
            series.iter().any(|v| v.is_finite() && *v != 0.0),
            "{name} is identically zero across the whole run, which is what a \
             dropped fragment looks like: {series:?}"
        );
    }
}

/// The VM oracle for the positional rule's other spelling: a bare numeric
/// literal into a NAMED dimension.
///
/// `compiler::subscript` lowers a constant index to `IndexOp::Single(value - 1)`
/// regardless of whether the axis is `Indexed` or `Named`, so `pop[2]` reads the
/// axis's SECOND element. The describers used to decline this (naming no
/// element, hence a conservative cross-product); they now resolve it, and this
/// test is what says which answer the simulation gives. Paired with
/// `ltm_agg::tests::classify_axis_access_resolves_a_colliding_name_element_first`,
/// whose `Expr(Const)` row asserts the classifier reaches the same element.
#[test]
fn numeric_literal_index_is_positional_in_a_named_dimension() {
    let project = TestProject::new("numeric_named")
        .named_dimension("Region", &["nyc", "boston"])
        .array_with_ranges_direct(
            "pop",
            vec!["Region".to_string()],
            vec![("nyc", "11"), ("boston", "22")],
            None,
        )
        .aux("picked", "pop[2]", None);

    assert_eq!(
        project.run_vm_expecting_success()["picked"].last().copied(),
        Some(22.0),
        "`pop[2]` must read Region's SECOND element (boston == 22)"
    );
}

/// The index's POSITION is load-bearing, and a 1-D fixture cannot show it.
///
/// `resolve_literal_index` resolves a constant against the axis at the index's
/// own position. Every other fixture here uses a 1-D source, where position is
/// always 0 and "the axis at this position" and "the source's first axis" are
/// the same thing -- so they would all still pass if the lookup ignored the
/// position entirely. This one separates them: `matrix[1,3]` needs axis 1
/// (`Wide`, 3 long) to resolve `3`, and reading axis 0 (`Narrow`, 2 long) finds
/// nothing and falls back to the conservative cross-product.
#[test]
fn a_constant_index_resolves_against_the_axis_at_its_own_position() {
    let project = TestProject::new("positional_axis")
        .named_dimension("Narrow", &["a", "b"])
        .named_dimension("Wide", &["p", "q", "r"])
        .array_with_ranges_direct(
            "matrix",
            vec!["Narrow".to_string(), "Wide".to_string()],
            vec![
                ("a,p", "11"),
                ("a,q", "12"),
                ("a,r", "13"),
                ("b,p", "21"),
                ("b,q", "22"),
                ("b,r", "23"),
            ],
            None,
        )
        .aux("picked", "matrix[1,3]", None);

    assert_eq!(
        project.run_vm_expecting_success()["picked"].last().copied(),
        Some(13.0),
        "`matrix[1,3]` must read Narrow's 1st and Wide's 3rd element"
    );

    let edges = element_edges(&project);
    assert_edge(&edges, "matrix[a,r]", "picked");
    for phantom in [
        "matrix[a,p]",
        "matrix[a,q]",
        "matrix[b,p]",
        "matrix[b,q]",
        "matrix[b,r]",
    ] {
        assert_no_edge(&edges, phantom, "picked");
    }
}

/// The positional rule narrows a reducer's read slice too, not just the element
/// graph -- `ltm_agg::resolve_literal_axis_index` is the second production
/// copy, and `compute_read_slice` is what consumes it.
///
/// `SUM(matrix[2,*])` used to leave axis 0 unclassifiable, which declined the
/// hoist and left the reference on the conservative cross-product: all four
/// source elements got an edge into `total`. The pinned axis now resolves, so
/// only the row the reducer actually reads is attributed.
#[test]
fn a_constant_pinned_reducer_axis_narrows_the_read_slice() {
    let project = TestProject::new("pinned_reducer_axis")
        .named_dimension("Row", &["a", "b"])
        .named_dimension("Col", &["p", "q"])
        .array_with_ranges_direct(
            "matrix",
            vec!["Row".to_string(), "Col".to_string()],
            vec![("a,p", "1"), ("a,q", "2"), ("b,p", "10"), ("b,q", "20")],
            None,
        )
        .aux("total", "SUM(matrix[2,*])", None);

    assert_eq!(
        project.run_vm_expecting_success()["total"].last().copied(),
        Some(30.0),
        "`SUM(matrix[2,*])` sums Row's SECOND element (b): 10 + 20"
    );

    let edges = element_edges(&project);
    assert_edge(&edges, "matrix[b,p]", "total");
    assert_edge(&edges, "matrix[b,q]", "total");
    assert_no_edge(&edges, "matrix[a,p]", "total");
    assert_no_edge(&edges, "matrix[a,q]", "total");
}

/// The emitter must produce a score for the instance's OWN element and for no
/// other -- the narrowing that is the whole point of the change.
///
/// Separate from
/// [`module_link_scores_are_per_target_element_and_hold_their_own_source_live`],
/// which checks that the score that SHOULD exist does and names the right
/// instance. That one passes unchanged if the emitter also mints
/// `…smth1⁚north→growth[south]`, and an extra per-element score is not inert:
/// `ltm_finding::parse_link_offsets` registers it as a discovery-graph edge
/// `(smth1⁚north, growth[south])`, which is exactly the phantom class this
/// change removes from the element graph.
#[test]
fn a_module_instance_scores_no_element_but_its_own() {
    let datamodel = smooth_loop_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    let emitted: Vec<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();
    for (instance, foreign) in [("north", "south"), ("south", "north")] {
        let phantom = format!("$⁚ltm⁚link_score⁚$⁚growth⁚0⁚smth1⁚{instance}→growth[{foreign}]");
        assert!(
            !emitted.contains(&phantom.as_str()),
            "the {instance} instance feeds only growth[{instance}], so {phantom} \
             names an edge that does not exist.\nemitted: {emitted:#?}"
        );
    }
}

/// A dimensioned capture helper is not element-bound. It is one
/// `Equation::ApplyToAll` array, carries no element suffix, and is referenced as
/// `helper[<elem>]`, so the element belongs on the reference and source side of
/// each score name. The per-element-module emitter sees it through the same
/// implicit namespace as scalar helpers; the production metadata dimensions
/// are the discriminator.
#[test]
fn an_arrayed_capture_helper_is_not_treated_as_element_bound() {
    // A real feedback loop, so LTM actually emits link scores here -- without a
    // stock this fixture emits none at all and every assertion below is vacuous.
    let project = TestProject::new("arrayed_capture_helper")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("Region", &["north", "south"])
        .array_stock("stock[Region]", "10", &["growth"], &[], None)
        .array_flow(
            "growth[Region]",
            "SMTH1(stock[Region], 1) * 0.1 + PREVIOUS(PREVIOUS(stock)) * 0.001",
            None,
        );
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let implicit =
        crate::db::model_implicit_var_info(&db, sync.models["main"].source_model, sync.project);
    let arrayed_helpers: Vec<&String> = implicit
        .iter()
        .filter(|(_, meta)| !meta.dimensions.is_empty())
        .map(|(name, _)| name)
        .collect();
    assert!(
        !arrayed_helpers.is_empty(),
        "fixture must synthesize an ARRAYED capture helper, else this test is \
         vacuous; implicit vars: {:?}",
        implicit.keys().collect::<Vec<_>>()
    );

    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
    for helper in &arrayed_helpers {
        let per_element_prefix = format!("$⁚ltm⁚link_score⁚{helper}→");
        // Positive half, and the discriminator: an ARRAYED source is scored with
        // the element on the FROM side (`{helper}[north]→growth`), which is the
        // arrayed-source treatment. The per-element emitter's signature is the
        // element on the TO side. Asserting the first exists is what keeps the
        // second's absence from being vacuous -- and it is what fails if the
        // guard is dropped, since this emitter runs before the one that produces
        // the from-side form and would claim the edge instead.
        let from_side = format!("$⁚ltm⁚link_score⁚{helper}[");
        assert!(
            ltm.vars
                .iter()
                .any(|v| v.name.starts_with(&from_side) && v.name.ends_with("→growth")),
            "{helper} must keep the ARRAYED-source treatment (element on the from \
             side).\nemitted: {:#?}",
            ltm.vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>()
        );
        let bracketed: Vec<&str> = ltm
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .filter(|n| n.starts_with(&per_element_prefix) && n.ends_with(']'))
            .collect();
        assert!(
            bracketed.is_empty(),
            "{helper} is a genuine array, not an element-bound scalar, so it must \
             not get per-target-element scores; got {bracketed:?}"
        );
    }
}

/// A dimensioned capture is one arrayed endpoint everywhere LTM observes it:
/// the element graph, link-score variables, loop topology, fragment dependency
/// shapes, layout, and simulation values all derive from its production
/// `ImplicitVarMeta::dimensions` row.
#[test]
fn an_arrayed_capture_endpoint_has_one_shape_through_ltm() {
    let project = TestProject::new("arrayed_capture_helper_compiles")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("Region", &["north", "south"])
        .array_stock("stock[Region]", "10", &["growth"], &[], None)
        .array_flow(
            "growth[Region]",
            "SMTH1(stock[Region], 1) * 0.1 + PREVIOUS(PREVIOUS(stock)) * 0.001",
            None,
        );
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let model = sync.models["main"].source_model;
    let implicit = crate::db::model_implicit_var_info(&db, model, sync.project);
    let helper = "$⁚growth⁚1⁚arg0";
    assert_eq!(implicit[helper].dimensions, ["region"]);

    let ltm = model_ltm_variables(&db, model, sync.project);
    let stock_to_capture = ltm
        .vars
        .iter()
        .find(|v| v.name == format!("$⁚ltm⁚link_score⁚stock→{helper}"))
        .expect("stock-to-capture score");
    assert_eq!(stock_to_capture.dimensions, ["Region"]);
    let capture_loop = ltm
        .vars
        .iter()
        .find(|v| v.name == "$⁚ltm⁚loop_score⁚u1")
        .expect("capture loop score");
    assert_eq!(capture_loop.dimensions, ["Region"]);
    assert!(
        ltm.vars.iter().all(|v| v.name != "$⁚ltm⁚loop_score⁚u2"),
        "the two compatible element loops share one dimensioned score"
    );

    let failures: Vec<String> = collect_all_diagnostics(&db, sync.project)
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Assembly(msg) if msg.contains("failed to compile") => {
                Some(format!("{:?}: {msg}", d.variable))
            }
            _ => None,
        })
        .collect();

    assert!(
        failures.is_empty(),
        "every score and loop touching the shaped capture must compile:\n{}",
        failures.join("\n")
    );

    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the LTM fixture must compile");
    let offset = compiled.offsets[&crate::common::Ident::new(capture_loop.name.as_str())];
    let mut vm = crate::vm::Vm::new(compiled).expect("VM creation");
    vm.run_to_end().expect("simulation");
    let results = vm.into_results();
    let slots: Vec<Vec<f64>> = (0..2)
        .map(|slot| {
            (0..results.step_count)
                .map(|step| results.data[step * results.step_size + offset + slot])
                .collect()
        })
        .collect();
    assert_eq!(slots[0], slots[1]);
    assert_eq!(slots[0].len(), 41);
    assert_eq!(&slots[0][..3], &[0.0, 0.0, 0.0]);
    assert!((slots[0][3] - 0.022346368715091925).abs() < 1e-15);
    assert!((slots[0][40] - 0.010328893875739318).abs() < 1e-15);
    assert!(
        slots[0]
            .iter()
            .any(|value| value.is_finite() && *value != 0.0),
        "a compiled capture loop must not read a zero-filled fallback: {slots:?}"
    );
}

/// A shaped capture can itself be the variable-backed result of a partial
/// reducer. The projection feeder into that hidden endpoint has only
/// per-row/per-slot score names, so tiered cycle classification must route the
/// loop through the element graph instead of composing a nonexistent Bare
/// score. The target shape comes from the same production implicit metadata as
/// the element graph and score emitter.
#[test]
fn shaped_capture_reducer_projection_feeder_uses_element_loops() {
    let project = TestProject::new("capture_projection_feeder")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux("matrix[D1,D2]", "1 + D1 + D2")
        .array_aux("frac[D1]", "1 + 0.01 * lagged[D1]")
        .array_aux("lagged[D1]", "PREVIOUS(SUM(matrix[D1, *] * frac[D1]), 0)");
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let model = sync.models["main"].source_model;
    let capture = "$⁚lagged⁚0⁚arg0";

    let implicit = crate::db::model_implicit_var_info(&db, model, sync.project);
    assert_eq!(
        implicit[capture].dimensions,
        ["d1"],
        "the real snapshot construction must retain the reducer result shape"
    );

    let shapes = model_edge_shapes(&db, model, sync.project);
    assert!(
        shapes
            .agg_routed_edges
            .contains(&("frac".to_string(), capture.to_string())),
        "the projection feeder into a hidden shaped reducer must avoid the Bare fast path: {:#?}",
        shapes.agg_routed_edges
    );

    let ltm = model_ltm_variables(&db, model, sync.project);
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|variable| variable.name.starts_with("$⁚ltm⁚loop_score⁚"))
        .collect();
    assert_eq!(
        loop_vars.len(),
        2,
        "the per-row score identities force two diagonal element circuits: {:?}",
        loop_vars
            .iter()
            .map(|variable| &variable.name)
            .collect::<Vec<_>>()
    );
    assert!(
        loop_vars
            .iter()
            .all(|variable| variable.dimensions.is_empty()),
        "these are scalar per-row circuits, not one grouped A2A loop"
    );
    for element in ["r1", "r2"] {
        let score = format!("$⁚ltm⁚link_score⁚frac[{element}]→{capture}[{element}]");
        assert!(
            ltm.vars.iter().any(|variable| variable.name == score),
            "the projection feeder has one real per-row score: {score}"
        );
        assert!(
            loop_vars
                .iter()
                .any(|variable| variable.equation.source_text().contains(&score)),
            "an element loop must consume the emitted score {score}: {:?}",
            loop_vars
                .iter()
                .map(|variable| variable.equation.source_text())
                .collect::<Vec<_>>()
        );
    }
    let nonexistent_bare = format!("\"$⁚ltm⁚link_score⁚frac→{capture}\"");
    assert!(
        loop_vars
            .iter()
            .all(|variable| !variable.equation.source_text().contains(&nonexistent_bare)),
        "no loop may reference the nonexistent Bare projection-feeder score"
    );

    let failures: Vec<String> = collect_all_diagnostics(&db, sync.project)
        .into_iter()
        .filter_map(|diagnostic| match diagnostic.error {
            DiagnosticError::Assembly(message) if message.contains("failed to compile") => {
                Some(message)
            }
            _ => None,
        })
        .collect();
    assert!(
        failures.is_empty(),
        "every per-row score and scalar loop must compile: {failures:#?}"
    );

    let observed_names: Vec<String> = ["r1", "r2"]
        .into_iter()
        .map(|element| format!("$⁚ltm⁚link_score⁚frac[{element}]→{capture}[{element}]"))
        .chain(loop_vars.iter().map(|variable| variable.name.clone()))
        .collect();
    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("projection-feeder LTM fixture must compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation");
    vm.run_to_end().expect("simulation");
    let results = vm.into_results();
    for name in observed_names {
        let offset = compiled.offsets[&crate::common::Ident::new(name.as_str())];
        let series: Vec<f64> = (0..results.step_count)
            .map(|step| results.data[step * results.step_size + offset])
            .collect();
        assert_eq!(
            series,
            [0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            "{name} must execute the exact per-row score rather than a zero stub"
        );
    }
}

/// An explicit Arrayed override does not consume the equation's default.
/// A shaped PREVIOUS capture used by that default remains one structural
/// helper, but only missing slots read it; broadcasting its read into the
/// override would manufacture a feedback loop and score that cannot affect
/// the simulated value.
#[test]
fn shaped_snapshot_default_scores_only_missing_elements() {
    let mut datamodel = TestProject::new("shaped_snapshot_default_topology")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("D", &["a", "b"])
        .array_aux("y[D]", "x[D] * 0.5 + 1")
        .build_datamodel();
    datamodel.models[0]
        .variables
        .push(crate::datamodel::Variable::Aux(crate::datamodel::Aux {
            ident: "x".to_string(),
            equation: crate::datamodel::Equation::Arrayed(
                vec!["D".to_string()],
                vec![("a".to_string(), "1".to_string(), None, None)],
                Some("PREVIOUS(y[D] * 2, 0)".to_string()),
                true,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: crate::datamodel::Compat::default(),
        }));
    let mut uid_by_name = std::collections::HashMap::new();
    for (index, variable) in datamodel.models[0].variables.iter_mut().enumerate() {
        let uid = index as i32 + 1;
        uid_by_name.insert(variable.get_ident().to_string(), uid);
        match variable {
            crate::datamodel::Variable::Stock(stock) => stock.uid = Some(uid),
            crate::datamodel::Variable::Flow(flow) => flow.uid = Some(uid),
            crate::datamodel::Variable::Aux(aux) => aux.uid = Some(uid),
            crate::datamodel::Variable::Module(module) => module.uid = Some(uid),
        }
    }
    datamodel.models[0]
        .loop_metadata
        .push(crate::datamodel::LoopMetadata {
            uids: vec![uid_by_name["x"], uid_by_name["y"]],
            deleted: false,
            name: "missing default slot".to_string(),
            description: String::new(),
        });

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let model = sync.models["main"].source_model;
    let capture = "$⁚x⁚0⁚arg0";
    let implicit = crate::db::model_implicit_var_info(&db, model, sync.project);
    assert_eq!(implicit[capture].dimensions, ["d"]);
    assert_eq!(
        implicit
            .keys()
            .filter(|name| name.as_str() == capture)
            .count(),
        1,
        "the missing slots share one structural capture"
    );

    let element_edges = model_element_causal_edges(&db, model, sync.project);
    assert!(
        element_edges.edges[&format!("{capture}[b]")].contains("x[b]"),
        "the missing slot consumes its capture"
    );
    assert!(
        !element_edges
            .edges
            .get(&format!("{capture}[a]"))
            .is_some_and(|targets| targets.contains("x[a]")),
        "the explicit override must not inherit the default's capture edge"
    );

    let edge_shapes = model_edge_shapes(&db, model, sync.project);
    assert!(
        edge_shapes
            .target_restricted_edges
            .contains(&(capture.to_string(), "x".to_string())),
        "the classifier must retain the missing-slot restriction: {edge_shapes:#?}"
    );
    let pinned = crate::db::model_pinned_loops(&db, model, sync.project);
    assert!(pinned.invalid.is_empty(), "the production pin is valid");
    assert_eq!(pinned.loops.len(), 1);
    assert_eq!(pinned.loops[0].loops.len(), 1);
    let pinned_loop = &pinned.loops[0].loops[0];
    assert!(
        pinned_loop.dimensions.is_empty()
            && pinned_loop.links.iter().all(|link| {
                link.from.as_str().contains("[b]") && link.to.as_str().contains("[b]")
            }),
        "the pin expands to the one real b-slot circuit: {pinned_loop:#?}"
    );

    let ltm = model_ltm_variables(&db, model, sync.project);
    let loop_vars: Vec<_> = ltm
        .vars
        .iter()
        .filter(|variable| variable.name.starts_with("$⁚ltm⁚loop_score⁚"))
        .collect();
    assert!(
        loop_vars
            .iter()
            .any(|variable| variable.equation.source_text().contains("[b]")),
        "the missing b slot's real lagged loop must be scored: {:?}",
        loop_vars
            .iter()
            .map(|variable| variable.equation.source_text())
            .collect::<Vec<_>>()
    );
    assert!(
        loop_vars
            .iter()
            .all(|variable| !variable.equation.source_text().contains("[a]")),
        "no loop may be emitted for overridden slot a: {:?}",
        loop_vars
            .iter()
            .map(|variable| variable.equation.source_text())
            .collect::<Vec<_>>()
    );

    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the shaped default and its LTM scores compile");
    let mut vm = crate::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("simulation");
    let results = vm.into_results();
    let x_a = results.offsets["x[a]"];
    let x_b = results.offsets["x[b]"];
    let series = |offset| {
        (0..results.step_count)
            .map(|step| results.data[step * results.step_size + offset])
            .collect::<Vec<_>>()
    };
    assert_eq!(series(x_a), [1.0; 5]);
    assert_eq!(series(x_b), [0.0, 2.0, 4.0, 6.0, 8.0]);
    for variable in loop_vars {
        let offset = results.offsets[variable.name.as_str()];
        let values = series(offset);
        assert_eq!(values[0], 0.0);
        assert!(
            values[1..].iter().all(|value| (*value - 1.0).abs() < 1e-12),
            "the isolated b loop score is exactly one after startup: {values:?}"
        );
    }
}

/// An Arrayed default whose apply flag is false is inert source text and must
/// not synthesize capture storage.
#[test]
fn inactive_snapshot_default_mints_no_capture() {
    let mut datamodel = TestProject::new("inactive_snapshot_default")
        .named_dimension("D", &["a", "b"])
        .array_const("y[D]", 1.0)
        .build_datamodel();
    datamodel.models[0]
        .variables
        .push(crate::datamodel::Variable::Aux(crate::datamodel::Aux {
            ident: "x".to_string(),
            equation: crate::datamodel::Equation::Arrayed(
                vec!["D".to_string()],
                vec![("a".to_string(), "1".to_string(), None, None)],
                Some("PREVIOUS(y[D] * 2, 0)".to_string()),
                false,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: crate::datamodel::Compat::default(),
        }));

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let implicit =
        crate::db::model_implicit_var_info(&db, sync.models["main"].source, sync.project);
    assert!(
        implicit.keys().all(|name| !name.starts_with("$⁚x⁚")),
        "an inactive default has no production capture: {:?}",
        implicit.keys().collect::<Vec<_>>()
    );
}

/// A default has no consumer when every declared storage slot has an explicit
/// body. Both helper-producing call families must remain absent even when the
/// apply flag is true: snapshot storage is shared only across real missing
/// slots, and module instances are inherently per missing slot.
#[test]
fn fully_overridden_defaults_mint_no_implicit_helpers() {
    for (label, default) in [
        ("snapshot", "PREVIOUS(y[D] * 2, 0)"),
        ("module", "SMTH1(y[D], 1, 0)"),
    ] {
        let mut datamodel = TestProject::new(label)
            .named_dimension("D", &["a", "b"])
            .array_const("y[D]", 1.0)
            .build_datamodel();
        datamodel.models[0]
            .variables
            .push(crate::datamodel::Variable::Aux(crate::datamodel::Aux {
                ident: "x".to_string(),
                equation: crate::datamodel::Equation::Arrayed(
                    vec!["D".to_string()],
                    vec![
                        ("a".to_string(), "1".to_string(), None, None),
                        ("b".to_string(), "2".to_string(), None, None),
                    ],
                    Some(default.to_string()),
                    true,
                ),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: crate::datamodel::Compat::default(),
            }));

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let implicit =
            crate::db::model_implicit_var_info(&db, sync.models["main"].source, sync.project);
        assert!(
            implicit.keys().all(|name| !name.starts_with("$⁚x⁚")),
            "{label}: a fully shadowed default has no production helper: {:?}",
            implicit.keys().collect::<Vec<_>>()
        );
    }
}

/// Dimensionless Arrayed input is a valid partially-edited XMILE shape. The
/// no-element-context fallback must honor an inactive default just like the
/// ordinary dimensioned branch does.
#[test]
fn dimensionless_inactive_defaults_mint_no_implicit_helpers() {
    for (label, default) in [
        ("snapshot", "PREVIOUS(1 + TIME, 0)"),
        ("module", "SMTH1(1 + TIME, 1, 0)"),
    ] {
        let mut datamodel = TestProject::new(label).build_datamodel();
        datamodel.models[0]
            .variables
            .push(crate::datamodel::Variable::Aux(crate::datamodel::Aux {
                ident: "x".to_string(),
                equation: crate::datamodel::Equation::Arrayed(
                    vec![],
                    vec![],
                    Some(default.to_string()),
                    false,
                ),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: crate::datamodel::Compat::default(),
            }));

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let implicit =
            crate::db::model_implicit_var_info(&db, sync.models["main"].source, sync.project);
        assert!(
            implicit.keys().all(|name| !name.starts_with("$⁚x⁚")),
            "{label}: inactive dimensionless source text must not enter the visitor: {:?}",
            implicit.keys().collect::<Vec<_>>()
        );
    }
}

/// A pathway link that cannot be resolved warns ONCE per edge, not once per
/// pathway that traverses it.
///
/// `enumerate_pathways_to_outputs` admits up to `MAX_PATHWAYS_PER_PORT` paths
/// per input port, a converging sub-model shares edges across most of them, and
/// `collect_model_diagnostics` does not deduplicate accumulator output. Without
/// the dedup this is one identical row per (pathway x link), which on a real
/// module graph buries every other diagnostic.
///
/// The fixture is the smallest shape with a SHARED unresolvable edge: the
/// sub-model's scalar input port feeds an ARRAYED reader (`mid[Region]`), which
/// is scored per target element (`input_val→mid[nyc]`), so the variable-level
/// pathway resolution cannot spell it -- and both output ports' pathways run
/// through that same edge.
#[test]
fn an_unresolved_pathway_link_warns_once_per_edge() {
    use crate::datamodel;
    use crate::testutils::{x_aux, x_model};

    let input = datamodel::Variable::Aux(datamodel::Aux {
        ident: "input_val".to_string(),
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
    let mid = datamodel::Variable::Aux(datamodel::Aux {
        ident: "mid".to_string(),
        equation: datamodel::Equation::ApplyToAll(
            vec!["Region".to_string()],
            "input_val * 2".to_string(),
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    // TWO output ports, so two pathways from `input_val`, both through `mid`.
    let sub = x_model(
        "diamond",
        vec![
            input,
            mid,
            x_aux("pos", "SUM(mid[*])", None),
            x_aux("neg", "0 - SUM(mid[*])", None),
        ],
    );

    let main = x_model(
        "main",
        vec![
            x_aux("drv", "1", None),
            datamodel::Variable::Module(datamodel::Module {
                ident: "m".to_string(),
                model_name: "diamond".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "drv".to_string(),
                    dst: "m.input_val".to_string(),
                }],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            x_aux("watcher", "m.pos + m.neg", None),
        ],
    );

    let mut project = crate::test_common::TestProject::new("pathway_dedup")
        .named_dimension("Region", &["nyc", "boston"])
        .build_datamodel();
    project.models = vec![main, sub];

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);

    let warnings: Vec<String> = collect_all_diagnostics(&db, sync.project)
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Assembly(msg) if msg.contains("LTM pathway score") => {
                Some(msg.clone())
            }
            _ => None,
        })
        .collect();

    let count_for = |edge: &str| -> usize {
        warnings
            .iter()
            .filter(|m| m.contains(&format!("for edge {edge}")))
            .count()
    };
    // `input_val -> mid` is traversed by BOTH pathways; `mid -> neg` by exactly
    // one. Equality is the assertion: it says the warning count does not scale
    // with pathway multiplicity, which is the whole point of the dedup. Without
    // it the counts are 4 and 2.
    //
    // Both are >1 because a sub-model's LTM diagnostics are collected twice --
    // once directly and once through the parent's subtree, since
    // `model_all_diagnostics` for `main` reaches `diamond`'s
    // `model_ltm_variables`. That doubling is uniform, pre-existing, and shared
    // by every LTM sub-model warning; the single-pathway edge doubling too is
    // what shows it cannot come from pathway multiplicity. Out of scope here.
    let shared = count_for("input_val -> mid");
    let single = count_for("mid -> neg");
    assert!(
        shared > 0 && single > 0,
        "fixture must produce both warnings, else this is vacuous:\n{}",
        warnings.join("\n")
    );
    assert_eq!(
        shared,
        single,
        "an edge traversed by two pathways must warn no more often than one \
         traversed by a single pathway; got {shared} vs {single}:\n{}",
        warnings.join("\n")
    );
}

/// Each target element's score must hold the port THAT element reads.
///
/// `module_output_ref_in_document_order` answers a per-EDGE question -- "which
/// output of this module does `to` read first" -- and a per-element emitter must
/// not reuse one answer for every slot. An `Ast::Arrayed` target may read a
/// different port of the same module in each slot (`x[a] = m·pos`,
/// `x[b] = m·neg`), and building `x[b]`'s partial with `m·pos` live means the
/// live source does not occur in that slot's body at all: the wrap has nothing
/// to hold live, and the denominator names a port the element never reads.
#[test]
fn each_element_scores_the_module_port_that_element_reads() {
    use crate::datamodel;
    use crate::testutils::{x_aux, x_model};

    let input = datamodel::Variable::Aux(datamodel::Aux {
        ident: "input_val".to_string(),
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
    let sub = x_model(
        "posneg",
        vec![
            input,
            x_aux("pos", "input_val * 2", None),
            x_aux("neg", "0 - input_val", None),
        ],
    );

    // Per-element equations reading DIFFERENT ports of the same instance.
    let x = datamodel::Variable::Aux(datamodel::Aux {
        ident: "x".to_string(),
        equation: datamodel::Equation::Arrayed(
            vec!["Region".to_string()],
            vec![
                ("a".to_string(), "m.pos".to_string(), None, None),
                ("b".to_string(), "m.neg".to_string(), None, None),
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
    // A stock, so `model_ltm_variables` does not take its stateless early return.
    let stock = datamodel::Variable::Stock(datamodel::Stock {
        ident: "s".to_string(),
        equation: datamodel::Equation::Scalar("1".to_string()),
        documentation: String::new(),
        units: None,
        inflows: vec!["f".to_string()],
        outflows: vec![],
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let main = x_model(
        "main",
        vec![
            x_aux("drv", "s * 0.1", None),
            datamodel::Variable::Module(datamodel::Module {
                ident: "m".to_string(),
                model_name: "posneg".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "drv".to_string(),
                    dst: "m.input_val".to_string(),
                }],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            x,
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "f".to_string(),
                equation: datamodel::Equation::Scalar("SUM(x[*])".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
            stock,
        ],
    );

    let mut project = crate::test_common::TestProject::new("per_element_port")
        .named_dimension("Region", &["a", "b"])
        .build_datamodel();
    project.models = vec![main, sub];

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);

    for (element, own, foreign) in [("a", "m·pos", "m·neg"), ("b", "m·neg", "m·pos")] {
        let name = format!("$⁚ltm⁚link_score⁚m→x[{element}]");
        let var = ltm.vars.iter().find(|v| v.name == name).unwrap_or_else(|| {
            let emitted: Vec<&str> = ltm.vars.iter().map(|v| v.name.as_str()).collect();
            panic!("expected {name}; emitted: {emitted:#?}")
        });
        let text = var.equation.source_text();
        assert!(
            text.contains(own),
            "x[{element}] reads {own}, so its score must hold that port live; \
             equation:\n{text}"
        );
        assert!(
            !text.contains(foreign),
            "x[{element}] does not read {foreign}; equation:\n{text}"
        );
    }
}
