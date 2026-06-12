// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for element-level causal graph expansion.
//!
//! Exercises `model_element_causal_edges` through the salsa pipeline,
//! verifying that variable-level edges are expanded correctly based on
//! dimension structure and dependency classification.
//!
//! Element names in the graph use canonical (lowercased) form because
//! `dimension_element_names` produces canonical names from `Dimension`.

// `model_element_loop_circuits` is `#[deprecated]` for new LTM callers;
// these tests legitimately drive the legacy element-Johnson surface to
// pin the element-graph topology contract.
#[allow(deprecated)]
use super::model_element_loop_circuits;
use super::{
    model_causal_edges, model_cycle_partitions, model_element_causal_edges,
    model_element_cycle_partitions, model_loop_circuits,
};
use crate::db::{SimlinDb, sync_from_datamodel};
use crate::test_common::TestProject;

/// Helper: build a TestProject, sync into salsa, and return the
/// element-level causal edges result for the "main" model.
fn element_edges(project: &TestProject) -> super::ElementCausalEdgesResult {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;
    model_element_causal_edges(&db, source_model, source_project).clone()
}

/// Helper: build a TestProject, sync into salsa, and return both
/// variable-level and element-level causal edges for comparison.
fn both_edge_levels(
    project: &TestProject,
) -> (super::CausalEdgesResult, super::ElementCausalEdgesResult) {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;
    let var_edges = model_causal_edges(&db, source_model, source_project).clone();
    let elem_edges = model_element_causal_edges(&db, source_model, source_project).clone();
    (var_edges, elem_edges)
}

/// Assert that `from_node` has an edge to `to_node` in the element graph.
fn assert_edge(result: &super::ElementCausalEdgesResult, from: &str, to: &str) {
    let targets = result.edges.get(from);
    assert!(
        targets.is_some_and(|ts| ts.contains(to)),
        "expected edge {from} -> {to}, but it was missing.\nedges from '{from}': {:?}",
        targets
    );
}

/// Assert that `from_node` does NOT have an edge to `to_node`.
fn assert_no_edge(result: &super::ElementCausalEdgesResult, from: &str, to: &str) {
    let has_edge = result.edges.get(from).is_some_and(|ts| ts.contains(to));
    assert!(
        !has_edge,
        "expected NO edge {from} -> {to}, but it was present"
    );
}

// ---- Test 1: AC2.7 (zero overhead for scalar models) ----

/// A model with no arrays should produce an element graph identical to the
/// variable graph. This verifies the zero-overhead fast path where
/// edges and stocks are cloned directly without any expansion.
#[test]
fn scalar_model_produces_identical_element_graph() {
    let project = TestProject::new("scalar_identity")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.1", None)
        .flow("deaths", "population * 0.05", None)
        .scalar_const("rate", 0.1);

    let (var_edges, elem_edges) = both_edge_levels(&project);

    // Edges should be identical (same keys, same value sets)
    assert_eq!(
        var_edges.edges, elem_edges.edges,
        "scalar model: element edges should be identical to variable edges"
    );

    // Stocks should be identical
    assert_eq!(
        var_edges.stocks, elem_edges.stocks,
        "scalar model: element stocks should be identical to variable stocks"
    );
}

// ---- Test 2: AC2.1 (same-dimension A2A edges) ----

/// An A2A model where both stock and flow share the same dimension
/// should produce per-element same-element edges.
///
/// population[Region] (stock) with births[Region] = population * 0.1 (flow)
///
/// The stock's inflow declarations create variable-level edges:
///   births -> population
/// which expand (SameElement) to:
///   births[nyc] -> population[nyc],  births[boston] -> population[boston], ...
///
/// The flow's equation `population * 0.1` creates:
///   population -> births
/// which expand (SameElement) to:
///   population[nyc] -> births[nyc],  population[boston] -> births[boston], ...
///
/// Element names are canonical (lowercased).
#[test]
fn same_dimension_a2a_expands_element_wise() {
    let project = TestProject::new("a2a_same_dim")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None);

    let result = element_edges(&project);

    // Flow -> Stock edges (from stock inflow declarations, SameElement)
    assert_edge(&result, "births[nyc]", "population[nyc]");
    assert_edge(&result, "births[boston]", "population[boston]");
    assert_edge(&result, "births[la]", "population[la]");

    // Stock -> Flow edges (from the flow's equation, SameElement)
    assert_edge(&result, "population[nyc]", "births[nyc]");
    assert_edge(&result, "population[boston]", "births[boston]");
    assert_edge(&result, "population[la]", "births[la]");

    // Cross-element edges should NOT exist (SameElement, not CrossElement)
    assert_no_edge(&result, "population[nyc]", "births[boston]");
    assert_no_edge(&result, "population[nyc]", "births[la]");
    assert_no_edge(&result, "births[nyc]", "population[boston]");
}

// ---- Test 3: AC2.2 (arrayed-to-scalar via reducer) ----

/// An arrayed variable reduced to a scalar via SUM should produce
/// element-to-scalar edges: each source element feeds the scalar target.
///
/// total_pop = SUM(population[*])
/// The population -> total_pop edge is CrossElement because of the wildcard.
/// Since total_pop is scalar, each population element feeds it.
#[test]
fn arrayed_to_scalar_via_sum() {
    let project = TestProject::new("a2s_sum")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_aux("population[Region]", "100")
        .scalar_aux("total_pop", "SUM(population[*])");

    let result = element_edges(&project);

    // Each population element feeds total_pop
    assert_edge(&result, "population[nyc]", "total_pop");
    assert_edge(&result, "population[boston]", "total_pop");
    assert_edge(&result, "population[la]", "total_pop");

    // total_pop should NOT have element subscripts (it's scalar)
    assert!(
        !result
            .edges
            .values()
            .any(|ts| ts.iter().any(|t| t.starts_with("total_pop["))),
        "total_pop should not appear with subscripts in any edge target"
    );
}

// ---- Test 4: AC2.3 (scalar-to-arrayed broadcast) ----

/// A scalar variable referenced in an A2A equation should produce
/// broadcast edges: the scalar feeds each target element.
///
/// births[Region] = population * growth_factor
/// growth_factor is scalar -> births[Region] is arrayed.
/// The scalar-to-arrayed edge becomes:
///   growth_factor -> births[nyc], growth_factor -> births[boston], ...
#[test]
fn scalar_to_arrayed_broadcast() {
    let project = TestProject::new("s2a_broadcast")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .scalar_const("growth_factor", 0.1)
        .array_aux("population[Region]", "100")
        .array_aux("births[Region]", "population * growth_factor");

    let result = element_edges(&project);

    // growth_factor (scalar) -> births[d] for each d (broadcast)
    assert_edge(&result, "growth_factor", "births[nyc]");
    assert_edge(&result, "growth_factor", "births[boston]");
    assert_edge(&result, "growth_factor", "births[la]");

    // Also verify the SameElement edges from population -> births
    assert_edge(&result, "population[nyc]", "births[nyc]");
    assert_edge(&result, "population[boston]", "births[boston]");
    assert_edge(&result, "population[la]", "births[la]");
}

// ---- Test 5: AC4.1 (reducer reference routed through an aggregate node) ----

/// An A2A variable whose equation contains `SUM(source[*])` routes that
/// reference through a synthetic aggregate node rather than emitting the
/// all-pairs Wildcard cross-product (Phase 5, design AC4.1).
///
/// `share[Region] = population / SUM(population[*])`: the maximal reducer
/// subexpression `SUM(population[*])` becomes `$⁚ltm⁚agg⁚0`, so the element
/// graph has `population[d] → $⁚ltm⁚agg⁚0` (one per source element),
/// `$⁚ltm⁚agg⁚0 → share[r]` (one per target element), and -- from the bare
/// `population / ...` numerator -- the diagonal `population[d] → share[d]`.
/// There is NO direct `population[d] → share[e]` Wildcard-derived edge.
#[test]
fn cross_element_wildcard_in_a2a() {
    let project = TestProject::new("cross_element_a2a")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_aux("population[Region]", "100")
        .array_aux("share[Region]", "population / SUM(population[*])");

    let result = element_edges(&project);

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let regions = &["nyc", "boston", "la"];

    // population[d] -> agg (the SUM reduction), one per source element.
    for &d in regions {
        assert_edge(&result, &format!("population[{d}]"), agg);
    }
    // agg -> share[r] (the broadcast), one per target element.
    for &r in regions {
        assert_edge(&result, agg, &format!("share[{r}]"));
    }
    // The bare numerator's diagonal edges remain.
    for &d in regions {
        assert_edge(&result, &format!("population[{d}]"), &format!("share[{d}]"));
    }
    // NO direct off-diagonal Wildcard cross-product edges.
    for &d in regions {
        for &r in regions {
            if d != r {
                assert_no_edge(&result, &format!("population[{d}]"), &format!("share[{r}]"));
            }
        }
    }
}

// ---- Test 6: AC2.5 (partial collapse with multi-dimensional arrays) ----

/// A 2D source array collapsed to a 1D target should produce partial
/// collapse edges: from[d1,d2] -> to[d1] for all combinations.
///
/// source[D1,D2] feeds target[D1] via a SameElement dependency.
/// Each 2D source element maps to the 1D target element sharing D1.
#[test]
fn partial_collapse_multi_dimensional() {
    let project = TestProject::new("partial_collapse")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct("source", vec!["D1".into(), "D2".into()], "10", None)
        .array_aux_direct("target", vec!["D1".into()], "source", None);

    let result = element_edges(&project);

    // source[d1,d2] -> target[d1] for all (d1, d2)
    assert_edge(&result, "source[a,x]", "target[a]");
    assert_edge(&result, "source[a,y]", "target[a]");
    assert_edge(&result, "source[b,x]", "target[b]");
    assert_edge(&result, "source[b,y]", "target[b]");

    // Cross-dimension edges should NOT exist
    assert_no_edge(&result, "source[a,x]", "target[b]");
    assert_no_edge(&result, "source[b,x]", "target[a]");
}

// ---- Test 7: AC2.6 (element-level stock nodes) ----

/// An arrayed stock should expand to element-level stock nodes.
/// The stocks set should contain per-element names (canonical/lowercased).
#[test]
fn arrayed_stock_expands_to_element_stock_nodes() {
    let project = TestProject::new("element_stocks")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None);

    let result = element_edges(&project);

    // Element-level stock nodes (canonical/lowercased)
    assert!(
        result.stocks.contains("population[nyc]"),
        "stocks should contain population[nyc], got: {:?}",
        result.stocks
    );
    assert!(
        result.stocks.contains("population[boston]"),
        "stocks should contain population[boston], got: {:?}",
        result.stocks
    );
    assert!(
        result.stocks.contains("population[la]"),
        "stocks should contain population[la], got: {:?}",
        result.stocks
    );

    // The bare variable name should NOT be in the stocks set (it's been expanded)
    assert!(
        !result.stocks.contains("population"),
        "stocks should not contain bare 'population', got: {:?}",
        result.stocks
    );

    // Should have exactly 3 stocks (one per region)
    assert_eq!(
        result.stocks.len(),
        3,
        "expected 3 element-level stocks, got {}: {:?}",
        result.stocks.len(),
        result.stocks
    );
}

// ---- Helpers for loop and partition tests ----

/// Helper: build a TestProject, sync into salsa, and return the
/// element-level loop circuits result for the "main" model.
///
/// Tests in this module call the legacy `model_element_loop_circuits`
/// directly to verify element-graph topology -- the diagnostic surface
/// these tests pin is exactly the unfiltered element-level circuit list,
/// not the post-#482 tiered output.
#[allow(deprecated)]
fn element_loop_circuits(project: &TestProject) -> super::LoopCircuitsResult {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;
    model_element_loop_circuits(&db, source_model, source_project).clone()
}

/// Helper: build a TestProject, sync into salsa, and return the
/// element-level cycle partitions result for the "main" model.
fn element_cycle_partitions(project: &TestProject) -> super::CyclePartitionsResult {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;
    model_element_cycle_partitions(&db, source_model, source_project).clone()
}

/// Helper: build a TestProject, sync into salsa, and return the
/// variable-level loop circuits result for the "main" model.
fn var_loop_circuits(project: &TestProject) -> super::LoopCircuitsResult {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;
    model_loop_circuits(&db, source_model, source_project).clone()
}

/// Helper: build a TestProject, sync into salsa, and return the
/// variable-level cycle partitions result for the "main" model.
fn var_cycle_partitions(project: &TestProject) -> super::CyclePartitionsResult {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;
    model_cycle_partitions(&db, source_model, source_project).clone()
}

/// Normalize a circuit for comparison: sort the node list rotation so
/// the lexicographically smallest node comes first, producing a canonical
/// representation independent of the starting node chosen by Johnson's
/// algorithm.
fn normalize_circuit(mut circuit: Vec<String>) -> Vec<String> {
    if circuit.is_empty() {
        return circuit;
    }
    // Find the position of the lexicographically smallest node
    let min_pos = circuit
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.cmp(b))
        .map(|(i, _)| i)
        .unwrap_or(0);
    // Rotate so that the smallest node comes first
    circuit.rotate_left(min_pos);
    circuit
}

/// Check that `circuits` contains a circuit matching `expected_nodes`
/// (after normalization of both).  Accepts the indexed `LoopCircuitsResult`
/// directly; materializing the owned-string view only happens for the
/// test assertion path.
fn assert_has_circuit(circuits: &super::LoopCircuitsResult, expected_nodes: &[&str]) {
    let expected: Vec<String> = expected_nodes.iter().map(|s| s.to_string()).collect();
    let normalized_expected = normalize_circuit(expected);

    let normalized_circuits: Vec<Vec<String>> = (0..circuits.len())
        .map(|i| normalize_circuit(circuits.circuit_names(i).map(String::from).collect()))
        .collect();

    assert!(
        normalized_circuits.contains(&normalized_expected),
        "expected circuit {:?} not found.\nactual circuits: {:?}",
        normalized_expected,
        normalized_circuits
    );
}

// ---- Test 8: AC3.1 (N element-identical loops for A2A model) ----

/// A model with `population[Region]` (stock, 3 regions) and `births[Region]`
/// (flow, `population * 0.1`) should produce exactly 3 element-level loops,
/// one per region. Each loop is a 2-node circuit: `[population[r], births[r]]`.
///
/// The same-element A2A feedback means each region's population only connects
/// to that region's births, so there are no cross-element loops.
#[test]
fn a2a_produces_n_element_identical_loops() {
    let project = TestProject::new("a2a_loops")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * 0.1", None);

    let result = element_loop_circuits(&project);

    // Should find exactly 3 loops (one per region)
    assert_eq!(
        result.circuits.len(),
        3,
        "expected 3 element-level loops, got {}: {:?}",
        result.circuits.len(),
        result.circuits
    );

    // Each loop should be a 2-node circuit: [population[r], births[r]]
    assert_has_circuit(&result, &["population[nyc]", "births[nyc]"]);
    assert_has_circuit(&result, &["population[boston]", "births[boston]"]);
    assert_has_circuit(&result, &["population[la]", "births[la]"]);
}

// ---- Test 9: AC4.1 (reducer reference routed through an aggregate node) ----

/// A model with `population[Region]` (stock, 2 regions) and `births[Region]`
/// (flow, `SUM(population[*]) * 0.01`) routes the inlined `SUM(population[*])`
/// through a synthetic aggregate node `$⁚ltm⁚agg⁚0` (Phase 5, design AC4.1).
///
/// The element graph then has `population[d] → $⁚ltm⁚agg⁚0` (per source
/// element), `$⁚ltm⁚agg⁚0 → births[r]` (per target element), and the
/// structural `births[r] → population[r]`. Johnson on this graph (the legacy
/// `model_element_loop_circuits` surface) finds two elementary circuits, one
/// per region, each routed through `$⁚ltm⁚agg⁚0`:
///   `[population[nyc], $⁚ltm⁚agg⁚0, births[nyc]]` and the Boston twin.
/// The genuine cross-element loop (`population[nyc] → agg → births[boston] →
/// population[boston] → agg → births[nyc] → ...`) visits `$⁚ltm⁚agg⁚0`
/// twice -- it is NOT an elementary circuit, so Johnson can't emit it
/// directly; the LTM loop builder reconstructs it (see the cross-element
/// loop tests in `db/ltm_unified_tests.rs`).
#[test]
fn cross_element_loop_through_sum_reducer() {
    let project = TestProject::new("cross_element_loop")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "SUM(population[*]) * 0.01", None);

    let result = element_loop_circuits(&project);

    // Two elementary circuits (one per region), each routed through the agg.
    assert_eq!(
        result.circuits.len(),
        2,
        "expected 2 agg-routed elementary circuits, got {}: {:?}",
        result.circuits.len(),
        result.circuits
    );
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    assert_has_circuit(&result, &["population[nyc]", agg, "births[nyc]"]);
    assert_has_circuit(&result, &["population[boston]", agg, "births[boston]"]);
}

// ---- Test 10: AC3.3 (partitions group cross-element stocks) ----

/// When stocks are connected through cross-element feedback (e.g., via
/// SUM(population[*])), they should be in the SAME partition because they
/// are mutually reachable through the causal graph.
#[test]
fn cross_element_stocks_in_same_partition() {
    let project = TestProject::new("cross_partition")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "SUM(population[*]) * 0.01", None);

    let result = element_cycle_partitions(&project);

    // Both population elements should be in the same partition
    let nyc_partition = result
        .stock_partition
        .get("population[nyc]")
        .expect("population[nyc] should be in a partition");
    let boston_partition = result
        .stock_partition
        .get("population[boston]")
        .expect("population[boston] should be in a partition");

    assert_eq!(
        nyc_partition, boston_partition,
        "population[nyc] (partition {nyc_partition}) and population[boston] (partition {boston_partition}) should be in the same partition"
    );

    // There should be exactly 1 partition containing both stocks
    assert_eq!(
        result.partitions.len(),
        1,
        "expected 1 partition, got {}: {:?}",
        result.partitions.len(),
        result.partitions
    );
}

// ---- Test 11: AC3.4 (separate partitions for independent stocks) ----

/// Two independent A2A stock-flow pairs with no cross-element connections
/// should produce separate partitions. Each element-level stock only
/// connects to its own flow element, so no stock is reachable from any
/// other stock.
#[test]
fn independent_stocks_in_separate_partitions() {
    let project = TestProject::new("separate_partitions")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("stock_a[Region]", "100", &["flow_a"], &[], None)
        .array_flow("flow_a[Region]", "stock_a * 0.1", None)
        .array_stock("stock_b[Region]", "50", &["flow_b"], &[], None)
        .array_flow("flow_b[Region]", "stock_b * 0.2", None);

    let result = element_cycle_partitions(&project);

    // All 4 element-level stocks should be in separate partitions because
    // each element's feedback is independent (SameElement only).
    assert_eq!(
        result.partitions.len(),
        4,
        "expected 4 partitions (one per element-level stock), got {}: {:?}",
        result.partitions.len(),
        result.partitions
    );

    // Each partition should contain exactly 1 stock
    for partition in &result.partitions {
        assert_eq!(
            partition.len(),
            1,
            "each partition should contain exactly 1 stock, got {:?}",
            partition
        );
    }

    // Verify that stock_a elements are in different partitions from stock_b elements
    let a_nyc = result
        .stock_partition
        .get("stock_a[nyc]")
        .expect("stock_a[nyc] should be in a partition");
    let a_boston = result
        .stock_partition
        .get("stock_a[boston]")
        .expect("stock_a[boston] should be in a partition");
    let b_nyc = result
        .stock_partition
        .get("stock_b[nyc]")
        .expect("stock_b[nyc] should be in a partition");
    let b_boston = result
        .stock_partition
        .get("stock_b[boston]")
        .expect("stock_b[boston] should be in a partition");

    // All four should be different
    let partitions = [a_nyc, a_boston, b_nyc, b_boston];
    for i in 0..partitions.len() {
        for j in (i + 1)..partitions.len() {
            assert_ne!(
                partitions[i], partitions[j],
                "element stocks should each be in separate partitions"
            );
        }
    }
}

// ---- Test 12: scalar model identity for loops and partitions ----

/// For a model with no arrays, `model_element_loop_circuits` and
/// `model_element_cycle_partitions` should produce results identical to
/// `model_loop_circuits` and `model_cycle_partitions`.
///
/// This verifies that the element-level analysis adds zero overhead for
/// scalar models and that the element-level graph is a faithful copy of
/// the variable-level graph.
#[test]
fn scalar_model_loops_and_partitions_identical() {
    let project = TestProject::new("scalar_loops")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.1", None)
        .flow("deaths", "population * 0.05", None)
        .scalar_const("rate", 0.1);

    // Compare loop circuits
    let var_circuits = var_loop_circuits(&project);
    let elem_circuits = element_loop_circuits(&project);

    // Normalize both for comparison (circuit ordering may differ).  The
    // indexed form doesn't produce owned strings, so materialize the
    // legacy shape once for the normalization dance.
    let mut var_normalized: Vec<Vec<String>> = var_circuits
        .to_named_circuits()
        .into_iter()
        .map(normalize_circuit)
        .collect();
    var_normalized.sort();
    let mut elem_normalized: Vec<Vec<String>> = elem_circuits
        .to_named_circuits()
        .into_iter()
        .map(normalize_circuit)
        .collect();
    elem_normalized.sort();

    assert_eq!(
        var_normalized, elem_normalized,
        "scalar model: element-level circuits should be identical to variable-level circuits"
    );

    // Compare cycle partitions
    let var_partitions = var_cycle_partitions(&project);
    let elem_partitions = element_cycle_partitions(&project);

    // Normalize partition ordering for comparison
    let mut var_parts: Vec<Vec<String>> = var_partitions.partitions;
    for p in &mut var_parts {
        p.sort();
    }
    var_parts.sort();
    let mut elem_parts: Vec<Vec<String>> = elem_partitions.partitions;
    for p in &mut elem_parts {
        p.sort();
    }
    elem_parts.sort();

    assert_eq!(
        var_parts, elem_parts,
        "scalar model: element-level partitions should be identical to variable-level partitions"
    );

    // stock_partition maps should also be identical
    assert_eq!(
        var_partitions.stock_partition, elem_partitions.stock_partition,
        "scalar model: element-level stock_partition should be identical to variable-level"
    );
}

// ---- Per-reference element-graph edge-set tests ----
//
// These tests pin the per-reference element-graph behavior. `model_element_causal_edges`
// walks each target's `Expr2` AST, classifies each reference by its
// `RefShape` (`Bare`, `FixedIndex`, `Wildcard`, or `DynamicIndex`), and
// emits edges per occurrence rather than per `(source, target)` pair. A
// `Wildcard`/`DynamicIndex` reference inside a maximal inlined reducer is
// rerouted through a synthetic `$⁚ltm⁚agg⁚{n}` aggregate node
// (`from[d] → agg`, `agg → to[e]`, O(N+M)) instead of the all-pairs N×M
// cross-product. The earlier classifier collapsed every reference into a
// single kind and over-approximated fixed-index references as a full N×N
// expansion; the asserted edge sets below reflect the truthful per-reference
// output.

/// AC1.1: Fixed-index broadcast must not be over-expanded.
///
/// For `relative_pop[Region] = population / population[NYC]` over a
/// dimension R of size N, the truthful element-graph contains exactly
/// the diagonal `population[d] -> relative_pop[d]` edges from the bare
/// `population` (SameElement) plus the broadcast-from-NYC edges
/// `population[nyc] -> relative_pop[d]` from the literal `population[NYC]`
/// (FixedIndex). After deduplication this is 2N - 1 = 5 edges for N=3,
/// not the N^2 = 9 edges today's classifier emits.
#[test]
fn element_graph_fixed_index_broadcast_truthful() {
    let project = TestProject::new("fixed_index_broadcast")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_aux("population[Region]", "100")
        .array_aux("relative_pop[Region]", "population / population[NYC]");

    let result = element_edges(&project);

    // Diagonal edges from the bare `population` (SameElement)
    assert_edge(&result, "population[nyc]", "relative_pop[nyc]");
    assert_edge(&result, "population[boston]", "relative_pop[boston]");
    assert_edge(&result, "population[la]", "relative_pop[la]");

    // Broadcast edges from the literal `population[NYC]` (FixedIndex).
    // The NYC-to-NYC edge overlaps with the diagonal edge above; only the
    // two non-overlapping ones add to the unique-edge count.
    assert_edge(&result, "population[nyc]", "relative_pop[boston]");
    assert_edge(&result, "population[nyc]", "relative_pop[la]");

    // Spurious edges that today's CrossElement-collapse classifier emits
    // and that the post-refactor builder must NOT emit. Each one would
    // imply that some element other than NYC is referenced from a literal
    // element subscript, which the equation does not do.
    assert_no_edge(&result, "population[boston]", "relative_pop[nyc]");
    assert_no_edge(&result, "population[la]", "relative_pop[nyc]");
    assert_no_edge(&result, "population[boston]", "relative_pop[la]");
    assert_no_edge(&result, "population[la]", "relative_pop[boston]");
}

/// AC4.1 (Phase 5): a Wildcard reducer reference is routed through a
/// synthetic aggregate node, and the bare-Var diagonal edges survive.
///
/// For `share[Region] = population / SUM(population[*])`, the maximal
/// reducer `SUM(population[*])` becomes `$⁚ltm⁚agg⁚0`. The truthful edge
/// set is N `population[d] → agg` + N `agg → share[r]` + N bare diagonals
/// `population[d] → share[d]` -- and explicitly NOT the N² Wildcard
/// cross-product the pre-Phase-5 classifier emitted.
#[test]
fn element_graph_wildcard_reducer_plus_bare_truthful() {
    let project = TestProject::new("wildcard_plus_bare")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_aux("population[Region]", "100")
        .array_aux("share[Region]", "population / SUM(population[*])");

    let result = element_edges(&project);

    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let regions = &["nyc", "boston", "la"];

    for &d in regions {
        assert_edge(&result, &format!("population[{d}]"), agg);
        assert_edge(&result, agg, &format!("share[{d}]"));
        assert_edge(&result, &format!("population[{d}]"), &format!("share[{d}]"));
    }
    for &d in regions {
        for &r in regions {
            if d != r {
                assert_no_edge(&result, &format!("population[{d}]"), &format!("share[{r}]"));
            }
        }
    }
}

/// AC4.1 (Phase 5): a whole-RHS *scalar* reducer (`total_pop = SUM(pop[*])`)
/// is its *own* aggregate node -- no `$⁚ltm⁚agg⁚{n}` is minted, and the
/// `pop[d] → total_pop` reduction edges plus the `total_pop → consumer`
/// broadcast edges already exist via the normal arrayed→scalar /
/// scalar→arrayed paths. The bare-numerator diagonal `pop[d] → migration[d]`
/// (from `... - pop[r] * c`) is the only direct `pop → migration` edge.
#[test]
fn element_graph_whole_rhs_scalar_reducer_is_its_own_agg_node() {
    let project = TestProject::new("whole_rhs_agg_graph")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["migration"], &[], None)
        .scalar_aux("total_pop", "SUM(pop[*])")
        .array_flow("migration[Region]", "total_pop * 0.001 - pop * 0.001", None);

    let result = element_edges(&project);

    // No synthetic agg node anywhere in the graph.
    assert!(
        !result
            .edges
            .keys()
            .any(|k| k.contains("\u{205A}agg\u{205A}"))
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t.contains("\u{205A}agg\u{205A}"))),
        "whole-RHS scalar reducer must not produce a synthetic agg node; got: {:?}",
        result.edges
    );

    // pop[d] -> total_pop reduction edges.
    assert_edge(&result, "pop[nyc]", "total_pop");
    assert_edge(&result, "pop[boston]", "total_pop");
    // total_pop -> migration[r] broadcast edges.
    assert_edge(&result, "total_pop", "migration[nyc]");
    assert_edge(&result, "total_pop", "migration[boston]");
    // Bare-numerator diagonal.
    assert_edge(&result, "pop[nyc]", "migration[nyc]");
    assert_edge(&result, "pop[boston]", "migration[boston]");
    // No off-diagonal pop -> migration edge (the only Wildcard ref of `pop`
    // is via `total_pop`, which is a real scalar node, not an N×M product).
    assert_no_edge(&result, "pop[nyc]", "migration[boston]");
    assert_no_edge(&result, "pop[boston]", "migration[nyc]");
}

/// GH #752 (element graph, variable-backed partial reduce): a whole-RHS
/// partial reducer (`inflow[D1] = SUM(matrix[D1,*])` is the ENTIRE
/// dt-equation, so `inflow` itself is the variable-backed agg --
/// `is_synthetic == false`, `result_dims = [D1]`, `read_slice =
/// [Iterated(d1), Reduced]`) gets read-slice-driven element edges, exactly
/// like a synthetic agg's source half: each `matrix[d1,d2]` row feeds ONLY
/// its own slot `inflow[d1]`. Before the fix the variable-backed reference
/// stayed on the conservative `Wildcard` cross-product, whose phantom
/// off-diagonal `matrix[a,x] -> inflow[b]` edges produced loop-score
/// equations referencing link-score names (`"matrix[a,x]→inflow"[b]`, the
/// bare A2A `"matrix→inflow"`) that `try_cross_dimensional_link_scores`
/// never emits -- only the per-`(row, slot)` scalars
/// `matrix[a,x]→inflow[a]` exist -- so every loop score through the
/// reducer failed fragment compile and was silently stubbed to 0.
#[test]
fn element_graph_variable_backed_partial_reduce_routes_read_slice() {
    let project = TestProject::new("vb_partial_reduce_elem_graph")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux("matrix[D1,D2]", "stock[D1] * 0.1")
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .array_flow("inflow[D1]", "SUM(matrix[D1,*])", None);

    let result = element_edges(&project);

    // No synthetic agg node: the variable IS the agg.
    assert!(
        !result
            .edges
            .keys()
            .any(|k| k.contains("\u{205A}agg\u{205A}"))
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t.contains("\u{205A}agg\u{205A}"))),
        "a variable-backed partial reducer must not produce a synthetic agg node; got: {:?}",
        result.edges
    );

    // The read-slice diagonal: each matrix row feeds only its own D1 slot.
    assert_edge(&result, "matrix[a,x]", "inflow[a]");
    assert_edge(&result, "matrix[a,y]", "inflow[a]");
    assert_edge(&result, "matrix[b,x]", "inflow[b]");
    assert_edge(&result, "matrix[b,y]", "inflow[b]");

    // No phantom off-diagonal cross-product edges: `SUM(matrix[a,*])` never
    // reads row `b` and vice versa.
    assert_no_edge(&result, "matrix[a,x]", "inflow[b]");
    assert_no_edge(&result, "matrix[a,y]", "inflow[b]");
    assert_no_edge(&result, "matrix[b,x]", "inflow[a]");
    assert_no_edge(&result, "matrix[b,y]", "inflow[a]");
}

/// GH #752 (two iterated axes): the read-slice routing also covers a
/// variable-backed partial reduce whose target iterates TWO axes
/// (`out[D1,D2] = SUM(cube[D1,D2,*])` over `cube[D1,D2,D3]`,
/// `read_slice = [Iterated(d1), Iterated(d2), Reduced]`, `result_dims =
/// [D1, D2]` equal to `out`'s dims in order): each cube row feeds only its
/// own `(d1, d2)` slot. Pins that `variable_backed_reduce_agg` keeps
/// admitting the all-`Iterated`/`Reduced` shapes byte-identically.
#[test]
fn element_graph_variable_backed_two_axis_partial_reduce_routes_read_slice() {
    let project = TestProject::new("vb_two_axis_partial_reduce")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "10",
            None,
        )
        .array_aux_direct(
            "out",
            vec!["D1".into(), "D2".into()],
            "SUM(cube[D1,D2,*])",
            None,
        );

    let result = element_edges(&project);

    // Diagonal slots only.
    assert_edge(&result, "cube[a,x,p]", "out[a,x]");
    assert_edge(&result, "cube[a,x,q]", "out[a,x]");
    assert_edge(&result, "cube[b,y,p]", "out[b,y]");
    assert_edge(&result, "cube[b,y,q]", "out[b,y]");
    // No cross-slot edges.
    assert_no_edge(&result, "cube[a,x,p]", "out[a,y]");
    assert_no_edge(&result, "cube[a,x,p]", "out[b,x]");
    assert_no_edge(&result, "cube[b,y,p]", "out[a,x]");
}

/// GH #765 (shape-expressiveness T3): a variable-backed partial reduce whose
/// read slice carries a PINNED axis (`outf[D1] = MEAN(cube[D1,x,*])` --
/// `read_slice = [Iterated(d1), Pinned(x), Reduced]`) takes the read-slice
/// routing: only the pinned `x` slab of each `D1` row feeds that row's slot.
/// `try_cross_dimensional_link_scores` now derives its per-`(row, slot)`
/// co-reduced slices from the same `read_slice_rows` (invariant I4), so the
/// edges and the emitted scores cover the identical read rows -- the T1-era
/// Pinned exclusion (which kept this shape on the loud conservative
/// cross-product while the score derivation ignored Pinned axes) is gone.
#[test]
fn element_graph_variable_backed_pinned_mixed_reduce_routes_read_slice() {
    let project = TestProject::new("vb_pinned_mixed_reduce")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "stock[D1] * 0.1",
            None,
        )
        .array_stock("stock[D1]", "10", &["inflow"], &[], None)
        .array_flow("inflow[D1]", "MEAN(cube[D1,x,*])", None);

    let result = element_edges(&project);

    // The read-slice diagonal: only the pinned x slab of each D1 row.
    assert_edge(&result, "cube[a,x,p]", "inflow[a]");
    assert_edge(&result, "cube[a,x,q]", "inflow[a]");
    assert_edge(&result, "cube[b,x,p]", "inflow[b]");
    assert_edge(&result, "cube[b,x,q]", "inflow[b]");

    // No unread-row edges (the y slab is pinned out) and no off-diagonal
    // cross-product edges.
    assert_no_edge(&result, "cube[a,y,p]", "inflow[a]");
    assert_no_edge(&result, "cube[a,y,q]", "inflow[a]");
    assert_no_edge(&result, "cube[b,y,p]", "inflow[b]");
    assert_no_edge(&result, "cube[a,x,p]", "inflow[b]");
    assert_no_edge(&result, "cube[b,x,p]", "inflow[a]");
}

/// Shape-expressiveness section 6 (scalar owner): a variable-backed
/// scalar-result Pinned slice (`total = SUM(pop[nyc,*])`) routes its element
/// edges by the read slice -- only the `pop[nyc,*]` rows feed the bare
/// `total` node (the slot of a scalar owner IS the owner). Pre-T3 the gate's
/// `to_dims.is_empty()` early-return kept the conservative full-extent
/// edges, whose unread-row scores were silent garbage.
#[test]
fn element_graph_variable_backed_scalar_owner_pinned_slice_routes_read_rows() {
    let project = TestProject::new("vb_scalar_owner_pinned")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct(
            "pop",
            vec!["Region".into(), "D2".into()],
            "stock[Region] * 0.1",
            None,
        )
        .scalar_aux("total", "SUM(pop[nyc, *])")
        .array_flow("inflow[Region]", "total * 0.05", None)
        .array_stock("stock[Region]", "100", &["inflow"], &[], None);

    let result = element_edges(&project);

    assert_edge(&result, "pop[nyc,p]", "total");
    assert_edge(&result, "pop[nyc,q]", "total");
    assert_no_edge(&result, "pop[boston,p]", "total");
    assert_no_edge(&result, "pop[boston,q]", "total");
}

/// Shape-expressiveness section 6 (scalar owner, subset slice):
/// `total = SUM(arr[*:Core])` over `arr[Region]` with `Core` a proper
/// subdimension -- only the subset rows feed `total`.
#[test]
fn element_graph_variable_backed_scalar_owner_subset_slice_routes_read_rows() {
    let project = TestProject::new("vb_scalar_owner_subset")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_aux("arr[Region]", "10")
        .scalar_aux("total", "SUM(arr[*:Core])");

    let result = element_edges(&project);

    assert_edge(&result, "arr[a]", "total");
    assert_edge(&result, "arr[b]", "total");
    assert_no_edge(&result, "arr[c]", "total");
}

/// GH #777: an ARRAYED-owner scalar-result Pinned slice
/// (`share[Region] = SUM(pop[nyc,*])` -- no `Iterated` axis, arrayed `to`)
/// fans the READ rows out across the FULL target element set: the single
/// scalar reducer value broadcasts over every `share[e]`, so each read row
/// `pop[nyc,*]` feeds every `share[e]`. The unread `pop[boston,*]` rows feed
/// NOTHING (the reducer never reads them). The edge set is EXACTLY (read
/// rows x all target elements) -- no unread-row edges, replacing the pre-fix
/// conservative cross-product superset.
#[test]
fn element_graph_variable_backed_broadcast_pinned_slice_fans_read_rows() {
    let project = TestProject::new("vb_broadcast_pinned")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct("pop", vec!["Region".into(), "D2".into()], "10", None)
        .array_aux_direct("share", vec!["Region".into()], "SUM(pop[nyc, *])", None);

    let result = element_edges(&project);

    // Read rows (pop[nyc,*]) x full target element set (share[nyc], share[boston]).
    for d2 in ["p", "q"] {
        for e in ["nyc", "boston"] {
            assert_edge(&result, &format!("pop[nyc,{d2}]"), &format!("share[{e}]"));
        }
    }
    // Unread rows (pop[boston,*]) feed no target slot, and the bare-name
    // dangling node never appears.
    for d2 in ["p", "q"] {
        for e in ["nyc", "boston"] {
            assert_no_edge(
                &result,
                &format!("pop[boston,{d2}]"),
                &format!("share[{e}]"),
            );
        }
        assert_no_edge(&result, &format!("pop[boston,{d2}]"), "share");
        assert_no_edge(&result, &format!("pop[nyc,{d2}]"), "share");
    }
}

/// AC4.1 (element graph, sliced reducer): `target[Region] = pop[NYC, Adult] +
/// SUM(pop[NYC, *])` over `pop[Region, Age]` (Region={NYC, Boston}, Age={Adult,
/// Child}). The maximal `SUM(pop[NYC, *])` subexpression is hoisted into a
/// synthetic agg `$⁚ltm⁚agg⁚0` whose `read_slice = [Pinned(nyc), Reduced]`, so
/// the element graph routes *only the NYC rows* through the agg:
/// `pop[nyc,adult] → agg`, `pop[nyc,child] → agg`, then `agg → target[r]`
/// for every `r` (the agg is scalar -- no `Iterated` axis -- so it
/// broadcasts). Boston's rows do *not* feed the agg, and there is no
/// `pop[d] → target[e]` full-cross-product edge from the reducer. The literal
/// `pop[NYC, Adult]` `FixedIndex` reference still broadcasts to every target.
#[test]
fn element_graph_sliced_reducer_reads_only_pinned_row() {
    let project = TestProject::new("sliced_reducer_elem_graph")
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["Adult", "Child"])
        .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "10", None)
        .array_aux_direct(
            "target",
            vec!["Region".into()],
            "pop[NYC, Adult] + SUM(pop[NYC, *])",
            None,
        );

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Only the NYC row feeds the agg (the slice reads `pop[NYC, *]`).
    assert_edge(&result, "pop[nyc,adult]", agg);
    assert_edge(&result, "pop[nyc,child]", agg);
    assert_no_edge(&result, "pop[boston,adult]", agg);
    assert_no_edge(&result, "pop[boston,child]", agg);

    // The scalar agg broadcasts to every target element.
    assert_edge(&result, agg, "target[nyc]");
    assert_edge(&result, agg, "target[boston]");

    // The literal `pop[NYC, Adult]` FixedIndex reference broadcasts.
    assert_edge(&result, "pop[nyc,adult]", "target[nyc]");
    assert_edge(&result, "pop[nyc,adult]", "target[boston]");

    // No `pop[d] → target[e]` full-cross-product edge from the reducer:
    // every reducer-side path goes via the agg, and only the NYC row at that.
    assert_no_edge(&result, "pop[nyc,child]", "target[nyc]");
    assert_no_edge(&result, "pop[nyc,child]", "target[boston]");
    assert_no_edge(&result, "pop[boston,adult]", "target[nyc]");
    assert_no_edge(&result, "pop[boston,adult]", "target[boston]");
    assert_no_edge(&result, "pop[boston,child]", "target[nyc]");
    assert_no_edge(&result, "pop[boston,child]", "target[boston]");
}

/// AC4.2 (element graph, arrayed agg over an iterated dim): `x[D1] =
/// matrix[a, x] + SUM(matrix[D1, *])` over `matrix[D1, D2]` (D1={a, b},
/// D2={x, y}), `x` apply-to-all over `D1`. The maximal `SUM(matrix[D1, *])`
/// subexpression -- a partial reduce keyed by the active A2A dimension `D1` --
/// is hoisted into an *arrayed* synthetic agg over `D1` (`read_slice =
/// [Iterated(d1), Reduced]`, `result_dims = [D1]`), so the element graph has
/// `matrix[d1, d2] → agg[d1]` (each `D1` row feeds that `D1`'s agg slot) and
/// `agg[d1] → x[d1]` (the diagonal projection), with NO `matrix[a, *] →
/// agg[b]` cross-slot edges. The literal `matrix[a, x]` FixedIndex reference
/// broadcasts to every `x` element.
#[test]
fn element_graph_arrayed_agg_over_iterated_dim() {
    let project = TestProject::new("arrayed_agg_elem_graph")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "x",
            vec!["D1".into()],
            "matrix[a, x] + SUM(matrix[D1, *])",
            None,
        );

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Each D1 row → that D1's agg slot.
    assert_edge(&result, "matrix[a,x]", &format!("{agg}[a]"));
    assert_edge(&result, "matrix[a,y]", &format!("{agg}[a]"));
    assert_edge(&result, "matrix[b,x]", &format!("{agg}[b]"));
    assert_edge(&result, "matrix[b,y]", &format!("{agg}[b]"));
    // No cross-slot edges (a's rows don't feed b's slot and vice versa).
    assert_no_edge(&result, "matrix[a,x]", &format!("{agg}[b]"));
    assert_no_edge(&result, "matrix[a,y]", &format!("{agg}[b]"));
    assert_no_edge(&result, "matrix[b,x]", &format!("{agg}[a]"));
    assert_no_edge(&result, "matrix[b,y]", &format!("{agg}[a]"));

    // The agg projects diagonally into `x`.
    assert_edge(&result, &format!("{agg}[a]"), "x[a]");
    assert_edge(&result, &format!("{agg}[b]"), "x[b]");
    assert_no_edge(&result, &format!("{agg}[a]"), "x[b]");
    assert_no_edge(&result, &format!("{agg}[b]"), "x[a]");

    // The literal `matrix[a, x]` FixedIndex reference broadcasts to every `x`.
    assert_edge(&result, "matrix[a,x]", "x[a]");
    assert_edge(&result, "matrix[a,x]", "x[b]");
    // No scalar (subscript-free) agg node -- the agg is arrayed over D1.
    assert!(
        !result.edges.contains_key(agg) && !result.edges.values().any(|ts| ts.contains(agg)),
        "the agg over D1 must always be subscripted (agg[d1]), never bare {agg}"
    );
}

/// #514 (element graph, a *mixed* `Iterated` + `Pinned` + `Reduced` read
/// slice): `matrix3d[D1, Region, Age]`, `x` A2A over `D1`, `x[D1] =
/// matrix3d[a, NYC, Adult] + SUM(matrix3d[D1, NYC, *])`. The reducer arg's
/// read slice is `[Iterated(d1), Pinned(nyc), Reduced]` (axis 0 iterated over
/// the target's `D1`, axis 1 pinned to literal `NYC`, axis 2 wildcard), so
/// the agg is arrayed over `D1` (`result_dims = [D1]`). The element graph
/// has `matrix3d[d1, nyc, age] → agg[d1]` (only the `NYC` slab of each `D1`
/// row, both `Age` elements) and `agg[d1] → x[d1]` (diagonal), with NO
/// `matrix3d[d1, boston, *]` edge into the agg and NO cross-`D1`-slot agg
/// edge. The literal `matrix3d[a, NYC, Adult]` FixedIndex reference
/// broadcasts to every `x` element.
#[test]
fn element_graph_mixed_pinned_iterated_reduced_slice() {
    let project = TestProject::new("mixed_slice_elem_graph")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["Adult", "Child"])
        .array_aux_direct(
            "matrix3d",
            vec!["D1".into(), "Region".into(), "Age".into()],
            "1",
            None,
        )
        .array_aux_direct(
            "x",
            vec!["D1".into()],
            "matrix3d[a, NYC, Adult] + SUM(matrix3d[D1, NYC, *])",
            None,
        );

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Only the NYC slab of each D1 row feeds that D1's agg slot (both Age elems).
    assert_edge(&result, "matrix3d[a,nyc,adult]", &format!("{agg}[a]"));
    assert_edge(&result, "matrix3d[a,nyc,child]", &format!("{agg}[a]"));
    assert_edge(&result, "matrix3d[b,nyc,adult]", &format!("{agg}[b]"));
    assert_edge(&result, "matrix3d[b,nyc,child]", &format!("{agg}[b]"));
    // The pinned `Region` axis: Boston rows never feed the agg.
    assert_no_edge(&result, "matrix3d[a,boston,adult]", &format!("{agg}[a]"));
    assert_no_edge(&result, "matrix3d[a,boston,child]", &format!("{agg}[a]"));
    assert_no_edge(&result, "matrix3d[b,boston,adult]", &format!("{agg}[b]"));
    assert_no_edge(&result, "matrix3d[b,boston,child]", &format!("{agg}[b]"));
    // No cross-D1-slot edges.
    assert_no_edge(&result, "matrix3d[a,nyc,adult]", &format!("{agg}[b]"));
    assert_no_edge(&result, "matrix3d[b,nyc,adult]", &format!("{agg}[a]"));

    // The agg projects diagonally into `x`.
    assert_edge(&result, &format!("{agg}[a]"), "x[a]");
    assert_edge(&result, &format!("{agg}[b]"), "x[b]");
    assert_no_edge(&result, &format!("{agg}[a]"), "x[b]");
    assert_no_edge(&result, &format!("{agg}[b]"), "x[a]");

    // The literal `matrix3d[a, NYC, Adult]` FixedIndex reference broadcasts.
    assert_edge(&result, "matrix3d[a,nyc,adult]", "x[a]");
    assert_edge(&result, "matrix3d[a,nyc,adult]", "x[b]");
    // No scalar (subscript-free) agg node -- the agg is arrayed over D1.
    assert!(
        !result.edges.contains_key(agg) && !result.edges.values().any(|ts| ts.contains(agg)),
        "the agg over D1 must always be subscripted (agg[d1]), never bare {agg}"
    );
}

/// AC4.4 (element graph, the dynamic-index carve-out): `x[Region] =
/// SUM(pop[idx, *])` over `pop[Region, Age]` with `idx` a scalar aux -- a
/// reducer over a dynamic index is *not* hoisted (`compute_read_slice`
/// declines the `idx` axis), so the IR reclassifies the `(pop, x)` reference
/// from `Wildcard` to `DynamicIndex` and the element graph keeps the
/// conservative `pop[d] → x[e]` full cross-product. No `$⁚ltm⁚agg` node
/// appears, and `pop` has no edge to any agg node.
#[test]
fn element_graph_dynamic_index_reducer_stays_conservative() {
    let project = TestProject::new("dyn_index_reducer_elem_graph")
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["Adult", "Child"])
        .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "10", None)
        .scalar_aux("idx", "1")
        .array_aux_direct("x", vec!["Region".into()], "SUM(pop[idx, *])", None);

    let result = element_edges(&project);

    // No synthetic agg node anywhere in the graph.
    assert!(
        !result
            .edges
            .keys()
            .any(|k| k.contains("\u{205A}agg\u{205A}"))
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t.contains("\u{205A}agg\u{205A}"))),
        "a dynamic-index reducer must not produce a synthetic agg node; got: {:?}",
        result.edges
    );

    // The conservative full cross-product: every `pop` element feeds every
    // `x` element.
    let from_elems = &[
        "pop[nyc,adult]",
        "pop[nyc,child]",
        "pop[boston,adult]",
        "pop[boston,child]",
    ];
    for from in from_elems {
        assert_edge(&result, from, "x[nyc]");
        assert_edge(&result, from, "x[boston]");
    }
}

/// #514 (element graph, *scalar* feeder of a hoisted *scalar* reducer): a
/// reducer whose argument references a *scalar* model variable, hoisted out of
/// an *arrayed* target -- `share[Region] = pop[Region] / SUM(pop[*] * scale)`
/// with `scale` scalar. The maximal `SUM(pop[*] * scale)` subexpression is
/// hoisted into a *scalar* synthetic agg `$⁚ltm⁚agg⁚0` (whole-extent reduce of
/// `pop[*]`, no iterated axis), and *both* feeders are routed through it: the
/// arrayed `pop[d] → agg` reductions and the scalar `scale → agg` edge. The
/// `scale` node must be the *bare* scalar name (`scale`), not the malformed
/// empty-bracket node `scale[]` the per-axis row machinery would produce when
/// fed an empty source-dimension list. The agg then broadcasts to every
/// `share` element. (`share`'s target must be arrayed -- with a *scalar*
/// target the `(scale, share)` edge would be short-circuited by
/// `model_element_causal_edges`'s both-scalar fast path before the IR's
/// `ThroughAgg` routing is consulted, so `emit_agg_routed_edges` would never
/// be reached with an empty `from_dims`.)
#[test]
fn element_graph_scalar_feeder_of_hoisted_reducer_is_bare_node() {
    let project = TestProject::new("scalar_feeder_hoisted_reducer")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("scale", "2")
        .array_aux("share[Region]", "pop / SUM(pop[*] * scale)");

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The scalar feeder is a *bare* node feeding the (scalar) agg -- not a
    // malformed `scale[]` bracketed node.
    assert_edge(&result, "scale", agg);
    assert!(
        !result.edges.contains_key("scale[]")
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t == "scale[]")),
        "the scalar feeder of a hoisted reducer must be the bare `scale` node, never `scale[]`; got: {:?}",
        result.edges
    );
    // The arrayed feeder reduces into the same scalar agg.
    assert_edge(&result, "pop[nyc]", agg);
    assert_edge(&result, "pop[boston]", agg);
    // The (scalar) agg broadcasts to every `share` element.
    assert_edge(&result, agg, "share[nyc]");
    assert_edge(&result, agg, "share[boston]");
    // The bare-numerator diagonal `pop[d] → share[d]` (from the `pop[Region] /`
    // numerator) survives.
    assert_edge(&result, "pop[nyc]", "share[nyc]");
    assert_edge(&result, "pop[boston]", "share[boston]");
}

/// #514 (element graph, scalar feeder of an *arrayed* hoisted reducer): a
/// sliced reducer over an A2A body whose argument also references a *scalar*,
/// e.g. `growth[D1] = SUM(matrix[D1,*] * scale)` with `scale` scalar -- the
/// reducer is hoisted into an *arrayed* agg over `D1` (`read_slice =
/// [Iterated(d1), Reduced]`, `result_dims = [D1]`). The scalar `scale` is not
/// subscripted, so it feeds *every* agg slot: `scale → agg[a]`, `scale →
/// agg[b]` -- and never the malformed `scale[]`. (`growth[D1] =
/// SUM(matrix[D1,*] * scale)` is a *whole-RHS* reducer -- a variable-backed
/// agg -- so `+ 1` keeps `SUM(...)` a sub-expression and forces a synthetic
/// agg.)
#[test]
fn element_graph_scalar_feeder_of_arrayed_hoisted_reducer_feeds_every_slot() {
    let project = TestProject::new("scalar_feeder_arrayed_reducer")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "10", None)
        .scalar_aux("scale", "2")
        .array_aux_direct(
            "growth",
            vec!["D1".into()],
            "SUM(matrix[D1,*] * scale) + 1",
            None,
        );

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The scalar feeder feeds every agg slot, as a bare node.
    assert_edge(&result, "scale", &format!("{agg}[a]"));
    assert_edge(&result, "scale", &format!("{agg}[b]"));
    assert!(
        !result.edges.contains_key("scale[]")
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t == "scale[]")),
        "the scalar feeder of an arrayed hoisted reducer must be the bare `scale` node, never `scale[]`; got: {:?}",
        result.edges
    );
    // The arrayed feeder routes each D1 row to that D1's agg slot.
    assert_edge(&result, "matrix[a,x]", &format!("{agg}[a]"));
    assert_edge(&result, "matrix[a,y]", &format!("{agg}[a]"));
    assert_edge(&result, "matrix[b,x]", &format!("{agg}[b]"));
    assert_edge(&result, "matrix[b,y]", &format!("{agg}[b]"));
    // The arrayed agg projects diagonally into `growth`.
    assert_edge(&result, &format!("{agg}[a]"), "growth[a]");
    assert_edge(&result, &format!("{agg}[b]"), "growth[b]");
}

// ---- #533: both-scalar fast path must not bypass ThroughAgg routing ----

/// #533 (the core scenario): a *scalar* feeder of a hoisted reducer whose
/// target is *also scalar* -- `total = base + SUM(pop[*] * scale)` with
/// `total`, `base`, `scale` scalar and `pop[*]` arrayed. The maximal
/// `SUM(pop[*] * scale)` subexpression is hoisted into a *scalar* synthetic agg
/// `$⁚ltm⁚agg⁚0` (whole-extent reduce of `pop[*]`, no iterated axis). The
/// `(scale, total)` causal edge is classified `ThroughAgg` in the IR, so it
/// must route `scale → $⁚ltm⁚agg⁚0` (then `$⁚ltm⁚agg⁚0 → total` exists via the
/// arrayed `pop` side) -- NOT a direct `scale → total` edge.
///
/// Before the fix, `model_element_causal_edges`'s both-scalar fast path
/// (`from_dims.is_empty() && to_dims.is_empty()`) short-circuited the
/// `(scale, total)` edge to a direct `scale → total` edge before the IR's
/// `ThroughAgg` routing was ever consulted, so the scalar-feeder→agg hop was
/// silently lost. (The `pop[d] → agg` and `agg → total` hops always existed,
/// since those edges aren't both-scalar.)
#[test]
fn element_graph_scalar_feeder_scalar_target_routes_through_agg() {
    let project = TestProject::new("scalar_feeder_scalar_target")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("scale", "2")
        .scalar_aux("base", "5")
        .scalar_aux("total", "base + SUM(pop[*] * scale)");

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The scalar feeder routes THROUGH the agg, not directly to the target.
    assert_edge(&result, "scale", agg);
    assert_no_edge(&result, "scale", "total");

    // The arrayed `pop` side wires the agg the same way it would for an
    // arrayed target: `pop[d] → agg` reductions and `agg → total` broadcast.
    assert_edge(&result, "pop[nyc]", agg);
    assert_edge(&result, "pop[boston]", agg);
    assert_edge(&result, agg, "total");

    // The scalar feeder must be the bare `scale` node, never a malformed
    // `scale[]` bracketed node (the `emit_agg_routed_edges` empty-`from_dims`
    // arm guards this once it is reached).
    assert!(
        !result.edges.contains_key("scale[]")
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t == "scale[]")),
        "the scalar feeder of a hoisted reducer must be the bare `scale` node, never `scale[]`; got: {:?}",
        result.edges
    );

    // The plain scalar `base + ...` term keeps its direct scalar edge: `base`
    // does not feed the reducer, so its `(base, total)` site is `Direct` and
    // the fast path's behavior is preserved for it.
    assert_edge(&result, "base", "total");
}

/// #533 (mixed Direct + ThroughAgg case): a scalar that feeds the scalar
/// target *both* directly and inside the reducer -- `total = scale +
/// SUM(pop[*] * scale)` with `total`, `scale` scalar and `pop[*]` arrayed. The
/// `(scale, total)` pair has TWO classified sites: one `Direct` (the bare
/// `scale +` term) and one `ThroughAgg` (the `scale` inside the reducer). Both
/// the direct `scale → total` edge AND the `scale → agg` hop must be emitted,
/// matching the normal (non-fast-path) dispatch.
#[test]
fn element_graph_scalar_feeder_mixed_direct_and_through_agg() {
    let project = TestProject::new("scalar_feeder_mixed")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("scale", "2")
        .scalar_aux("total", "scale + SUM(pop[*] * scale)");

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The `ThroughAgg` site routes `scale → agg`.
    assert_edge(&result, "scale", agg);
    // The `Direct` site (the bare `scale +` term) keeps the direct edge.
    assert_edge(&result, "scale", "total");
    // The arrayed `pop` side wires the agg.
    assert_edge(&result, "pop[nyc]", agg);
    assert_edge(&result, "pop[boston]", agg);
    assert_edge(&result, agg, "total");
}

/// #533 (fast path preserved): a plain scalar→scalar edge with no reducer
/// anywhere still takes the both-scalar direct-edge fast path -- `b = a * 2`
/// with both scalar emits a direct `a → b` edge and never invents a synthetic
/// agg node. This pins that consulting the IR for the ThroughAgg case does not
/// regress the common scalar→scalar `Direct` `Bare` case.
#[test]
fn element_graph_plain_scalar_to_scalar_keeps_direct_fast_path() {
    // The model must contain at least one arrayed variable, otherwise the
    // whole-model `any_arrayed` short-circuit returns the variable graph
    // verbatim and the per-edge loop (with its fast path) is never entered.
    let project = TestProject::new("plain_scalar_fast_path")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("unrelated[Region]", "1")
        .scalar_aux("a", "10")
        .scalar_aux("b", "a * 2");

    let result = element_edges(&project);

    // The plain scalar→scalar edge is direct.
    assert_edge(&result, "a", "b");
    // No synthetic agg node is minted for a reducer-free scalar→scalar edge.
    assert!(
        !result
            .edges
            .keys()
            .any(|k| k.contains("\u{205A}agg\u{205A}"))
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t.contains("\u{205A}agg\u{205A}"))),
        "a reducer-free scalar→scalar edge must not produce a synthetic agg node; got: {:?}",
        result.edges
    );
}

/// #533 (loop level): a feedback loop that runs *through* the scalar-feeder→agg
/// hop -- `total` (scalar stock) is grown by `grow = 1 + SUM(pop[*] * scale)`,
/// and `scale` feeds back from `total`. The cross-element-invisible loop
/// `total → scale → $⁚ltm⁚agg⁚0 → grow → total` must surface at the element
/// level *with the agg node in the circuit*. Before the fix the both-scalar
/// fast path replaced `scale → $⁚ltm⁚agg⁚0` with a direct `scale → grow`, so
/// the loop -- if found at all -- routed around the agg, breaking the
/// loop-score chain that walks `… → scale → $⁚ltm⁚agg⁚0 → grow → …`.
///
/// `SUM(pop[*] * scale)` must be a *sub-expression* (here `1 + ...`) so it is
/// hoisted into a *synthetic* `$⁚ltm⁚agg⁚0`; a whole-RHS reducer would be a
/// variable-backed agg (the flow itself) with no synthetic node, and the
/// `(scale, grow)` edge would stay `Direct`.
#[test]
fn element_graph_scalar_feeder_loop_routes_through_agg() {
    let project = TestProject::new("scalar_feeder_loop")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("scale", "0.001 * total")
        .stock("total", "100", &["grow"], &[], None)
        .flow("grow", "1 + SUM(pop[*] * scale)", None);

    let result = element_loop_circuits(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The feedback loop must route through the synthetic agg node. The
    // scalar-feeder→agg hop (`scale → agg`) is the segment the fast path used
    // to bypass; with it restored the loop visits the agg.
    assert_has_circuit(&result, &["total", "scale", agg, "grow"]);
}

// ---- #511: iterated-dimension subscript -> same-element projection ----

/// AC3.1 (element-graph side): an A2A target that references an arrayed
/// dependency by its *iterated dimension* (`growth[Region,Age] =
/// row_sum[Region] * c`, `row_sum` over `Region`, `growth` over
/// `Region x Age`) classifies the `row_sum[Region]` subscript as `Bare`
/// (see `ltm_ir_tests::ir_iterated_dim_subscript_is_bare`), so the
/// element graph has the same-element-on-shared-dims projection
/// `row_sum[r] -> growth[r,a]` for every `(r, a)` -- and NOT the full
/// `row_sum[r1] -> growth[r2,*]` cross-product. Before the fix the subscript
/// classified as `DynamicIndex` and emitted the all-pairs cross-product.
#[test]
fn element_graph_iterated_dim_subscript_same_element_projection() {
    let project = TestProject::new("iterated_dim_elem_graph")
        .named_dimension("Region", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("row_sum[Region]", "100")
        .array_aux_direct(
            "growth",
            vec!["Region".into(), "Age".into()],
            "row_sum[Region] * 0.5",
            None,
        );

    let result = element_edges(&project);

    // Same-element-on-shared-dims: row_sum[r] feeds growth[r,young] and
    // growth[r,old] -- broadcasting over the target-only dimension Age.
    assert_edge(&result, "row_sum[a]", "growth[a,young]");
    assert_edge(&result, "row_sum[a]", "growth[a,old]");
    assert_edge(&result, "row_sum[b]", "growth[b,young]");
    assert_edge(&result, "row_sum[b]", "growth[b,old]");

    // No cross-region edges (the reference is same-element on Region, not a
    // full cross-product).
    assert_no_edge(&result, "row_sum[a]", "growth[b,young]");
    assert_no_edge(&result, "row_sum[a]", "growth[b,old]");
    assert_no_edge(&result, "row_sum[b]", "growth[a,young]");
    assert_no_edge(&result, "row_sum[b]", "growth[a,old]");
}

/// AC3.5 / GH #527: a mapped-dimension iterated subscript (`x` over `Region`,
/// `target` over `State`, a `State→Region` mapping, `target[State] =
/// x[State] * c`) classifies `x[State]` as `Bare` (see
/// `ltm_ir_tests::ir_mapped_iterated_dim_subscript_is_bare`), and
/// `expand_same_element` projects it along the mapping's element
/// correspondence: with the positional `State→Region` mapping (s1↦a, s2↦b)
/// the element graph is the DIAGONAL `x[a]→target[s1]`, `x[b]→target[s2]` --
/// NOT the `Region × State` cross-product the pre-#527 name-only matching
/// broadcast to. The bare form (`target[State] = x * c`) resolves through
/// the same compiler mapping correspondence, so it must produce the
/// identical diagonal.
#[test]
fn element_graph_mapped_iterated_dim_matches_bare_baseline() {
    let subscripted = TestProject::new("mapped_iterated_subscripted")
        .named_dimension("Region", &["a", "b"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux_direct("x", vec!["Region".into()], "100", None)
        .array_aux_direct("target", vec!["State".into()], "x[State] * 0.5", None);
    let bare = TestProject::new("mapped_iterated_bare")
        .named_dimension("Region", &["a", "b"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux_direct("x", vec!["Region".into()], "100", None)
        .array_aux_direct("target", vec!["State".into()], "x * 0.5", None);

    let sub_result = element_edges(&subscripted);
    let bare_result = element_edges(&bare);

    assert_eq!(
        sub_result.edges, bare_result.edges,
        "a mapped-dimension iterated subscript `x[State]` must produce the \
         same element edges as a bare `x` reference into the same target"
    );

    // The positional mapping's diagonal, and nothing else.
    assert_edge(&sub_result, "x[a]", "target[s1]");
    assert_edge(&sub_result, "x[b]", "target[s2]");
    assert_no_edge(&sub_result, "x[a]", "target[s2]");
    assert_no_edge(&sub_result, "x[b]", "target[s1]");
}

/// GH #527: the mapping declared in the REVERSE direction -- on the
/// *source's* dimension (`Region→State`) rather than the target equation's
/// iterated dimension. A bare `x` reference still classifies `Bare` (no
/// subscript to inspect), and the compiler resolves it through
/// `translate_via_mapping`'s forward branch, so the expansion projects the
/// same diagonal.
#[test]
fn element_graph_mapped_reverse_declared_bare_is_diagonal() {
    let project = TestProject::new("mapped_reverse_bare")
        .named_dimension_with_mapping("Region", &["a", "b"], "State")
        .named_dimension("State", &["s1", "s2"])
        .array_aux_direct("x", vec!["Region".into()], "100", None)
        .array_aux_direct("target", vec!["State".into()], "x * 0.5", None);

    let result = element_edges(&project);

    assert_edge(&result, "x[a]", "target[s1]");
    assert_edge(&result, "x[b]", "target[s2]");
    assert_no_edge(&result, "x[a]", "target[s2]");
    assert_no_edge(&result, "x[b]", "target[s1]");
}

/// GH #757 (flipped from the GH #527-era conservative pin): a SUBSCRIPTED
/// iterated-dim reference whose POSITIONAL mapping is declared only in the
/// reverse direction (`Region→State`, i.e. on the source's dimension) now
/// classifies `Bare` -- `classify_iterated_dim_shape`'s mapped arm gates on
/// the same `mapped_element_correspondence` data `expand_same_element`
/// consults (both declaration directions), so the subscripted form gets the
/// same DIAGONAL the bare form (`element_graph_mapped_reverse_declared_bare_is_diagonal`)
/// already got, matching the compiler's `translate_via_mapping` (which
/// resolves both directions identically).
#[test]
fn element_graph_mapped_reverse_declared_subscripted_is_diagonal() {
    let project = TestProject::new("mapped_reverse_subscripted")
        .named_dimension_with_mapping("Region", &["a", "b"], "State")
        .named_dimension("State", &["s1", "s2"])
        .array_aux_direct("x", vec!["Region".into()], "100", None)
        .array_aux_direct("target", vec!["State".into()], "x[State] * 0.5", None);

    let result = element_edges(&project);

    assert_edge(&result, "x[a]", "target[s1]");
    assert_edge(&result, "x[b]", "target[s2]");
    assert_no_edge(&result, "x[a]", "target[s2]");
    assert_no_edge(&result, "x[b]", "target[s1]");
}

/// GH #525 (T6): a mixed iterated+literal subscript (`pop[Region, young]`
/// inside an A2A-over-`Region` equation) classifies `PerElement` and the
/// element graph emits ONLY the diagonal-with-pinned-axes edges -- the
/// same-`Region` element pinned at `Age = young` -- never the conservative
/// cross-product (`pop[a,*] -> row_sum[b]`) whose phantom circuits carried
/// silent confident loop scores.
#[test]
fn element_graph_per_element_mixed_subscript_is_pinned_diagonal() {
    let project = TestProject::new("per_element_mixed")
        .named_dimension("Region", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "100", None)
        .array_aux_direct(
            "row_sum",
            vec!["Region".into()],
            "pop[Region, young] + pop[Region, old]",
            None,
        );

    let result = element_edges(&project);

    // The two sites' pinned diagonals, and nothing else.
    assert_edge(&result, "pop[a,young]", "row_sum[a]");
    assert_edge(&result, "pop[a,old]", "row_sum[a]");
    assert_edge(&result, "pop[b,young]", "row_sum[b]");
    assert_edge(&result, "pop[b,old]", "row_sum[b]");
    assert_no_edge(&result, "pop[a,young]", "row_sum[b]");
    assert_no_edge(&result, "pop[a,old]", "row_sum[b]");
    assert_no_edge(&result, "pop[b,young]", "row_sum[a]");
    assert_no_edge(&result, "pop[b,old]", "row_sum[a]");
}

/// GH #525 (T6, broadcast): a `PerElement` reference whose Iterated dims
/// are a strict SUBSET of the target's (`mid[D1,D2] = pop[D1, young] *
/// 0.05` -- `D1` iterated, `Age` pinned, `D2` broadcast) emits one edge per
/// (row, full target element): the row feeds every `D2` slot of its `D1`
/// row, never another `D1` row, and the unpinned `Age` elements feed
/// nothing.
#[test]
fn element_graph_per_element_broadcast_is_pinned_diagonal() {
    let project = TestProject::new("per_element_broadcast")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("Age", &["young", "old"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct("pop", vec!["D1".into(), "Age".into()], "100", None)
        .array_aux_direct(
            "mid",
            vec!["D1".into(), "D2".into()],
            "pop[D1, young] * 0.05",
            None,
        );

    let result = element_edges(&project);

    assert_edge(&result, "pop[a,young]", "mid[a,x]");
    assert_edge(&result, "pop[a,young]", "mid[a,y]");
    assert_edge(&result, "pop[b,young]", "mid[b,x]");
    assert_edge(&result, "pop[b,young]", "mid[b,y]");
    // No cross-D1 edges, and the unread `old` rows feed nothing.
    assert_no_edge(&result, "pop[a,young]", "mid[b,x]");
    assert_no_edge(&result, "pop[b,young]", "mid[a,x]");
    assert_no_edge(&result, "pop[a,old]", "mid[a,x]");
    assert_no_edge(&result, "pop[a,old]", "mid[a,y]");
    assert_no_edge(&result, "pop[b,old]", "mid[b,x]");
}

/// GH #527: an EXPLICIT element-level mapping (here the different-
/// cardinality many-to-one `State{s1,s2,s3}→Region{a,b}`: s1↦a, s2↦a,
/// s3↦b) keeps the conservative BROADCAST, not a map-following diagonal:
/// the engine's executed A2A lowering resolves mapped references
/// positionally, ignoring the element map (this 3→2 model doesn't compile
/// at all -- GH #753 -- so the graph-level pin is all we can have here;
/// the same positional-execution inconsistency is why
/// `mapped_element_correspondence` declines element maps wholesale, see
/// its rustdoc gate). The broadcast is a superset of whatever the engine
/// would read, so no true edge can be missing.
#[test]
fn element_graph_mapped_element_map_stays_broadcast() {
    let project = TestProject::new("mapped_element_map_3_to_2")
        .named_dimension("Region", &["a", "b"])
        .named_dimension_with_element_mapping(
            "State",
            &["s1", "s2", "s3"],
            "Region",
            &[("s1", "a"), ("s2", "a"), ("s3", "b")],
        )
        .array_aux_direct("x", vec!["Region".into()], "100", None)
        .array_aux_direct("target", vec!["State".into()], "x[State] * 0.5", None);

    let result = element_edges(&project);

    for region in ["a", "b"] {
        for state in ["s1", "s2", "s3"] {
            assert_edge(
                &result,
                &format!("x[{region}]"),
                &format!("target[{state}]"),
            );
        }
    }
}

/// GH #527: the mapped diagonal composes with the target-only-dimension
/// broadcast. `target[State,Age] = x[State] * c` (`x` over `Region`,
/// `State→Region` positional mapping): the State axis projects along the
/// mapping while the Age axis broadcasts -- `x[a]` feeds `target[s1,*]`
/// only, never `target[s2,*]`.
#[test]
fn element_graph_mapped_diagonal_with_broadcast_dim() {
    let project = TestProject::new("mapped_diag_broadcast")
        .named_dimension("Region", &["a", "b"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .named_dimension("Age", &["young", "old"])
        .array_aux_direct("x", vec!["Region".into()], "100", None)
        .array_aux_direct(
            "target",
            vec!["State".into(), "Age".into()],
            "x[State] * 0.5",
            None,
        );

    let result = element_edges(&project);

    assert_edge(&result, "x[a]", "target[s1,young]");
    assert_edge(&result, "x[a]", "target[s1,old]");
    assert_edge(&result, "x[b]", "target[s2,young]");
    assert_edge(&result, "x[b]", "target[s2,old]");
    assert_no_edge(&result, "x[a]", "target[s2,young]");
    assert_no_edge(&result, "x[a]", "target[s2,old]");
    assert_no_edge(&result, "x[b]", "target[s1,young]");
    assert_no_edge(&result, "x[b]", "target[s1,old]");
}

/// Graph-vs-SIMULATION parity for an ASYMMETRIC explicit element map.
///
/// `Region{a,b}`, `State{s1,s2}` with the PERMUTED element map s1↦b, s2↦a,
/// distinct per-element source values (`x[a]=10`, `x[b]=20`), and
/// `target[State] = x[State] * 1`. The element graph's edges must be a
/// SUPERSET of the edges the simulation's actual reads imply (derived here
/// from which `x` element's value each `target` element equals) -- the LTM
/// contract is "more edges than necessary, never fewer".
///
/// Today the engine's executed A2A lowering resolves mapped references
/// POSITIONALLY, ignoring the explicit element map (target[s1] = x[a], the
/// map notwithstanding; see GH #753 for the related different-cardinality
/// compile failure), so a map-following diagonal would DROP the true
/// positionally-read edges -- which is why `mapped_element_correspondence`
/// declines explicit element maps (conservative broadcast). The test
/// derives the implied edges from the run itself, so it keeps passing if
/// the engine later honors element maps in execution and the element-map
/// diagonal is re-enabled.
#[test]
fn element_graph_mapped_element_map_edges_superset_of_simulation_reads() {
    let project = TestProject::new("mapped_element_map_parity")
        .named_dimension("Region", &["a", "b"])
        .named_dimension_with_element_mapping(
            "State",
            &["s1", "s2"],
            "Region",
            &[("s1", "b"), ("s2", "a")],
        )
        .array_with_ranges_direct(
            "x",
            vec!["Region".into()],
            vec![("a", "10"), ("b", "20")],
            None,
        )
        .array_aux_direct("target", vec!["State".into()], "x[State] * 1", None);

    let sim = project.run_vm_incremental();
    let graph = element_edges(&project);

    let region_elems = ["a", "b"];
    for state_elem in ["s1", "s2"] {
        let target_key = format!("target[{state_elem}]");
        let target_val = sim
            .get(&target_key)
            .unwrap_or_else(|| panic!("{target_key} missing from sim results"))[0];
        // The source element the simulation ACTUALLY read for this target
        // element (source values are distinct, so the match is unique).
        let read_from: Vec<&str> = region_elems
            .iter()
            .copied()
            .filter(|r| (sim[&format!("x[{r}]")][0] - target_val).abs() < 1e-9)
            .collect();
        assert_eq!(
            read_from.len(),
            1,
            "exactly one x element should match {target_key}={target_val}"
        );
        assert_edge(&graph, &format!("x[{}]", read_from[0]), &target_key);
    }
}

/// GH #527: two dimensions with the same shape but NO declared mapping must
/// keep the conservative broadcast -- the mapped diagonal only applies when
/// the model declares the correspondence.
#[test]
fn element_graph_unmapped_disjoint_dims_stay_broadcast() {
    let project = TestProject::new("unmapped_disjoint_broadcast")
        .named_dimension("Region", &["a", "b"])
        .named_dimension("State", &["s1", "s2"])
        .array_aux_direct("x", vec!["Region".into()], "100", None)
        .array_aux_direct("target", vec!["State".into()], "x * 0.5", None);

    let result = element_edges(&project);

    assert_edge(&result, "x[a]", "target[s1]");
    assert_edge(&result, "x[a]", "target[s2]");
    assert_edge(&result, "x[b]", "target[s1]");
    assert_edge(&result, "x[b]", "target[s2]");
}

/// GH #534: a positionally-MAPPED sliced reducer subexpression
/// (`SUM(matrix[State,*])` inside an A2A-over-`State` body, `matrix` over
/// `[Region,D2]`, positional `State→Region` mapping) is hoisted into an
/// arrayed synthetic agg over `State`, and the element graph routes only the
/// read rows through it WITH the slot remapped along the mapping: source
/// rows over `Region` land on the agg slot of the corresponding `State`
/// element (s1↦r1, s2↦r2), then `agg[s] → growth[s]` diagonally. No
/// cross-slot row edges, and no direct `matrix → growth` edges (the
/// reference is fully routed through the agg).
#[test]
fn element_graph_mapped_sliced_reducer_routes_through_remapped_agg() {
    let project = TestProject::new("mapped_sliced_agg")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "growth",
            vec!["State".into()],
            "1 + SUM(matrix[State, *])",
            None,
        );

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Source rows route to the agg slot of the MAPPED State element.
    assert_edge(&result, "matrix[r1,x]", &format!("{agg}[s1]"));
    assert_edge(&result, "matrix[r1,y]", &format!("{agg}[s1]"));
    assert_edge(&result, "matrix[r2,x]", &format!("{agg}[s2]"));
    assert_edge(&result, "matrix[r2,y]", &format!("{agg}[s2]"));
    // Never the cross slot.
    assert_no_edge(&result, "matrix[r1,x]", &format!("{agg}[s2]"));
    assert_no_edge(&result, "matrix[r2,x]", &format!("{agg}[s1]"));
    // The agg fans into the target diagonally on the shared State axis.
    assert_edge(&result, &format!("{agg}[s1]"), "growth[s1]");
    assert_edge(&result, &format!("{agg}[s2]"), "growth[s2]");
    assert_no_edge(&result, &format!("{agg}[s1]"), "growth[s2]");
    // The reducer reference is fully routed through the agg: no direct
    // matrix → growth element edges remain.
    for r in ["r1", "r2"] {
        for d2 in ["x", "y"] {
            for s in ["s1", "s2"] {
                assert_no_edge(
                    &result,
                    &format!("matrix[{r},{d2}]"),
                    &format!("growth[{s}]"),
                );
            }
        }
    }
}

/// GH #534 (conservative gate): a sliced reducer over an EXPLICIT
/// element-mapped pair stays un-hoisted -- the engine's executed A2A
/// lowering resolves mapped references positionally, ignoring element maps
/// (GH #756), so `mapped_element_correspondence` declines and the reference
/// keeps the conservative full cross-product (a superset of the true reads).
/// No agg node appears in the element graph.
#[test]
fn element_graph_element_mapped_sliced_reducer_stays_cross_product() {
    let project = TestProject::new("element_mapped_sliced")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_element_mapping(
            "State",
            &["s1", "s2"],
            "Region",
            &[("s1", "r2"), ("s2", "r1")],
        )
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "growth",
            vec!["State".into()],
            "1 + SUM(matrix[State, *])",
            None,
        );

    let result = element_edges(&project);

    // Conservative cross-product: every matrix row feeds every growth slot.
    for r in ["r1", "r2"] {
        for d2 in ["x", "y"] {
            for s in ["s1", "s2"] {
                assert_edge(
                    &result,
                    &format!("matrix[{r},{d2}]"),
                    &format!("growth[{s}]"),
                );
            }
        }
    }
    // No agg node was minted for the element-mapped sliced reducer.
    assert!(
        !result
            .edges
            .keys()
            .any(|k| k.starts_with("$\u{205A}ltm\u{205A}agg\u{205A}")),
        "an element-mapped sliced reducer must not route through an agg node; edges: {:?}",
        result.edges.keys().collect::<Vec<_>>()
    );
}

/// GH #757 (flipped from the GH #534-era conservative pin): a sliced
/// reducer whose POSITIONAL mapping is declared only in the REVERSE
/// direction (on the source's `Region` toward `State`) is now hoisted --
/// `classify_axis_access` gates on `mapped_element_correspondence`, which
/// accepts both declaration directions -- so the element graph routes it
/// through the remapped agg slots exactly like the forward-declared twin.
#[test]
fn element_graph_reverse_declared_mapped_sliced_reducer_routes_remapped_agg() {
    let project = TestProject::new("reverse_mapped_sliced")
        .named_dimension_with_mapping("Region", &["r1", "r2"], "State")
        .named_dimension("D2", &["x", "y"])
        .named_dimension("State", &["s1", "s2"])
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "growth",
            vec!["State".into()],
            "1 + SUM(matrix[State, *])",
            None,
        );

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Source rows route to the agg slot of the positionally-corresponding
    // State element; the agg fans into the target diagonally.
    assert_edge(&result, "matrix[r1,x]", &format!("{agg}[s1]"));
    assert_edge(&result, "matrix[r1,y]", &format!("{agg}[s1]"));
    assert_edge(&result, "matrix[r2,x]", &format!("{agg}[s2]"));
    assert_edge(&result, "matrix[r2,y]", &format!("{agg}[s2]"));
    assert_no_edge(&result, "matrix[r1,x]", &format!("{agg}[s2]"));
    assert_no_edge(&result, "matrix[r2,x]", &format!("{agg}[s1]"));
    assert_edge(&result, &format!("{agg}[s1]"), "growth[s1]");
    assert_edge(&result, &format!("{agg}[s2]"), "growth[s2]");
    assert_no_edge(&result, &format!("{agg}[s1]"), "growth[s2]");
    // No conservative direct matrix → growth edges remain.
    for r in ["r1", "r2"] {
        for d2 in ["x", "y"] {
            for s in ["s1", "s2"] {
                assert_no_edge(
                    &result,
                    &format!("matrix[{r},{d2}]"),
                    &format!("growth[{s}]"),
                );
            }
        }
    }
}

/// GH #534 (scalar co-feeder composition): a mapped sliced reducer with a
/// scalar co-feeder (`SUM(matrix[State,*] * scale)`) routes the scalar
/// feeder to EVERY remapped agg slot (a scalar can't pick a slot) while the
/// arrayed rows keep the mapping's slot diagonal.
#[test]
fn element_graph_mapped_sliced_reducer_with_scalar_cofeeder() {
    let project = TestProject::new("mapped_sliced_cofeeder")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .scalar_aux("scale", "2")
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "growth",
            vec!["State".into()],
            "1 + SUM(matrix[State, *] * scale)",
            None,
        );

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // Arrayed rows: the mapping's slot diagonal.
    assert_edge(&result, "matrix[r1,x]", &format!("{agg}[s1]"));
    assert_edge(&result, "matrix[r2,y]", &format!("{agg}[s2]"));
    assert_no_edge(&result, "matrix[r1,x]", &format!("{agg}[s2]"));
    // Scalar co-feeder: every agg slot.
    assert_edge(&result, "scale", &format!("{agg}[s1]"));
    assert_edge(&result, "scale", &format!("{agg}[s2]"));
}

/// GH #766 (element graph): an inline reducer over a *proper subdimension*
/// StarRange (`x = 1 + MEAN(arr[*:Core])`, `Core = {a, b}` a proper
/// subdimension of `Region = {a, b, c}`) is hoisted with a SUBSET-bearing
/// `Reduced` axis, so only the subdimension's rows feed the agg --
/// `arr[c]` (outside the subset) gets no edge. Pre-fix the slice claimed
/// the full parent extent and `arr[c] → agg` was a spurious edge (loop
/// enumeration could discover loops through a row the reducer never reads).
#[test]
fn element_graph_subset_star_range_reads_only_subdimension_rows() {
    let project = TestProject::new("gh766_subset_elem_graph")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_aux("arr[Region]", "10")
        .scalar_aux("x", "1 + MEAN(arr[*:Core])");

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    assert_edge(&result, "arr[a]", agg);
    assert_edge(&result, "arr[b]", agg);
    assert_no_edge(&result, "arr[c]", agg);
    assert_edge(&result, agg, "x");
    // No direct reducer-side edges bypassing the agg.
    assert_no_edge(&result, "arr[a]", "x");
    assert_no_edge(&result, "arr[c]", "x");
}

/// GH #778/#785: a DEGENERATE SQUARE source whose iterated axes carry the
/// SAME target dim twice -- `out[D1] = base[D1] + SUM(cube[D1, D1, *])` over
/// `cube[D1, D1, D2]`. The hoist is now DECLINED at minting
/// (`ltm_agg::result_dims_has_repeated_dim`), so NO `$⁚ltm⁚agg⁚{n}` node is
/// minted and the reducer's `cube` reference stays on the conservative
/// `DynamicIndex` cross-product (`emit_edges_for_reference`).
///
/// This test was previously a golden pin (`*_routes_full_cartesian`) that
/// deliberately recorded the now-fixed phantom: the agg→out
/// `expand_same_element` fan-out emitted the off-diagonal `agg[r1, r2] →
/// out[r2]` while the link-score projection kept only the diagonal, minting
/// warned 0-stub loops (#778), and the co-source row partials scored the
/// phantom off-diagonal rows the simulation never reads (#785). Declining the
/// hoist makes all of that disappear: the element graph keeps the sound
/// (coarse) cross-product, and the score side loudly skips the edge
/// (`emit_unscoreable_duplicated_dim_source_warning`) so loops through it drop
/// rather than reference never-emitted names.
#[test]
fn element_graph_square_source_duplicated_dim_declines_to_cross_product() {
    let project = TestProject::new("square_source_elem_graph")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux("cube[D1,D1,D2]", "1")
        .array_aux("base[D1]", "0")
        .array_aux("out[D1]", "base[D1] + SUM(cube[D1, D1, *])");

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // No agg node is minted at all (the square-source hoist is declined), so
    // no node name carries the agg prefix on either side of any edge --
    // including SUBSCRIPTED slot nodes like `{agg}[r1,r2]`, which an
    // exact-name membership check would miss.
    assert!(
        !result.edges.keys().any(|k| k.contains(agg))
            && !result
                .edges
                .values()
                .any(|ts| ts.iter().any(|t| t.contains(agg))),
        "the declined square-source reducer must mint NO agg node; found {agg} in the graph"
    );

    // `cube` routes as the conservative cross-product: every `cube` element
    // feeds every `out` element (a sound superset of the true diagonal reads).
    for ri in ["r1", "r2"] {
        for rj in ["r1", "r2"] {
            for ck in ["c1", "c2"] {
                for target in ["out[r1]", "out[r2]"] {
                    assert_edge(&result, &format!("cube[{ri},{rj},{ck}]"), target);
                }
            }
        }
    }

    // `base[ri]` keeps its diagonal same-element edge into `out[ri]`.
    assert_edge(&result, "base[r1]", "out[r1]");
    assert_edge(&result, "base[r2]", "out[r2]");
    assert_no_edge(&result, "base[r1]", "out[r2]");
}

/// Golden pin for the I4 single-derivation refactor (GH #783): an
/// ITERATED-DIM PROJECTION FEEDER (`frac[D1]` in `out[D1] = base[D1] +
/// SUM(matrix[D1, *] * frac[D1])`) carries its OWN all-`Iterated` projection
/// slice (one axis, `Iterated(D1)`), distinct from the canonical reduced
/// source `matrix[D1, *]` whose slice is `[Iterated(D1), Reduced]`. The
/// source→agg row enumeration the #783 refactor unifies must route the feeder
/// by its own slice (1:1 row→slot, `frac[ri] → agg[ri]`) -- never the
/// canonical source's slice -- so this exercises the per-source-slice path of
/// the shared derivation.
#[test]
fn element_graph_projection_feeder_routes_by_own_slice() {
    let project = TestProject::new("projection_feeder_elem_graph")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux("matrix[D1,D2]", "1")
        .array_aux("frac[D1]", "2")
        .array_aux("base[D1]", "0")
        .array_aux("out[D1]", "base[D1] + SUM(matrix[D1, *] * frac[D1])");

    let result = element_edges(&project);
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";

    // The canonical reduced source: `matrix[ri, *]` row routes to slot `[ri]`.
    assert_edge(&result, "matrix[r1,c1]", &format!("{agg}[r1]"));
    assert_edge(&result, "matrix[r1,c2]", &format!("{agg}[r1]"));
    assert_edge(&result, "matrix[r2,c1]", &format!("{agg}[r2]"));
    assert_edge(&result, "matrix[r2,c2]", &format!("{agg}[r2]"));
    assert_no_edge(&result, "matrix[r1,c1]", &format!("{agg}[r2]"));

    // The projection feeder: routed by its OWN all-Iterated slice, 1:1
    // row→slot. `frac[ri]` feeds ONLY `agg[ri]` -- never every slot (that
    // would be the scalar-feeder broadcast) and never the cross slot.
    assert_edge(&result, "frac[r1]", &format!("{agg}[r1]"));
    assert_edge(&result, "frac[r2]", &format!("{agg}[r2]"));
    assert_no_edge(&result, "frac[r1]", &format!("{agg}[r2]"));
    assert_no_edge(&result, "frac[r2]", &format!("{agg}[r1]"));

    // The agg fans diagonally into `out`.
    assert_edge(&result, &format!("{agg}[r1]"), "out[r1]");
    assert_edge(&result, &format!("{agg}[r2]"), "out[r2]");
    assert_no_edge(&result, &format!("{agg}[r1]"), "out[r2]");
}
