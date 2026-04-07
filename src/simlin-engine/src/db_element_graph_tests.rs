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

use super::{
    model_causal_edges, model_cycle_partitions, model_element_causal_edges,
    model_element_cycle_partitions, model_element_loop_circuits, model_loop_circuits,
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

// ---- Test 5: AC2.4 (cross-element via wildcard in A2A equation) ----

/// An A2A variable whose equation contains SUM(source[*]) should produce
/// cross-element edges: every source element connects to every target element.
///
/// share[Region] = population / SUM(population[*])
/// The population -> share dependency is CrossElement because of the
/// wildcard subscript in SUM(population[*]).
/// Each population element feeds every share element.
#[test]
fn cross_element_wildcard_in_a2a() {
    let project = TestProject::new("cross_element_a2a")
        .named_dimension("Region", &["NYC", "Boston", "LA"])
        .array_aux("population[Region]", "100")
        .array_aux("share[Region]", "population / SUM(population[*])");

    let result = element_edges(&project);

    // CrossElement: every population[d] -> every share[e]
    let regions = &["nyc", "boston", "la"];
    for &from_r in regions {
        for &to_r in regions {
            assert_edge(
                &result,
                &format!("population[{from_r}]"),
                &format!("share[{to_r}]"),
            );
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
/// (after normalization of both).
fn assert_has_circuit(circuits: &[Vec<String>], expected_nodes: &[&str]) {
    let expected: Vec<String> = expected_nodes.iter().map(|s| s.to_string()).collect();
    let normalized_expected = normalize_circuit(expected);

    let normalized_circuits: Vec<Vec<String>> = circuits
        .iter()
        .map(|c| normalize_circuit(c.clone()))
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
    assert_has_circuit(&result.circuits, &["population[nyc]", "births[nyc]"]);
    assert_has_circuit(&result.circuits, &["population[boston]", "births[boston]"]);
    assert_has_circuit(&result.circuits, &["population[la]", "births[la]"]);
}

// ---- Test 9: AC3.2 (cross-element loop detection) ----

/// A model with `population[Region]` (stock, 2 regions) and `births[Region]`
/// (flow, `SUM(population[*]) * 0.01`) should produce both same-element and
/// cross-element loops.
///
/// The SUM(population[*]) creates CrossElement edges, meaning every population
/// element feeds every births element. Combined with the SameElement stock
/// inflows (births[r] -> population[r]), this produces:
///
/// - 2 same-element loops: [population[nyc], births[nyc]] and
///   [population[boston], births[boston]]
/// - 1 cross-element loop: a 4-node circuit through both regions,
///   e.g., [population[nyc], births[boston], population[boston], births[nyc]]
#[test]
fn cross_element_loop_through_sum_reducer() {
    let project = TestProject::new("cross_element_loop")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "SUM(population[*]) * 0.01", None);

    let result = element_loop_circuits(&project);

    // Should find 3 loops total: 2 same-element + 1 cross-element
    assert_eq!(
        result.circuits.len(),
        3,
        "expected 3 circuits (2 same-element + 1 cross-element), got {}: {:?}",
        result.circuits.len(),
        result.circuits
    );

    // Same-element loops
    assert_has_circuit(&result.circuits, &["population[nyc]", "births[nyc]"]);
    assert_has_circuit(&result.circuits, &["population[boston]", "births[boston]"]);

    // Cross-element loop: 4-node circuit through both regions
    assert_has_circuit(
        &result.circuits,
        &[
            "population[nyc]",
            "births[boston]",
            "population[boston]",
            "births[nyc]",
        ],
    );
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

    // Normalize both for comparison (circuit ordering may differ)
    let mut var_normalized: Vec<Vec<String>> = var_circuits
        .circuits
        .into_iter()
        .map(normalize_circuit)
        .collect();
    var_normalized.sort();
    let mut elem_normalized: Vec<Vec<String>> = elem_circuits
        .circuits
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
