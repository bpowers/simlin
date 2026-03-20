// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Structural compiler tests for array-producing builtin hoisting (A2A).
//!
//! These tests verify that VectorSortOrder, VectorElmMap, and AllocateAvailable
//! are hoisted into AssignTemp pre-computations when used in array (A2A) context,
//! rather than being evaluated redundantly per element.

use simlin_engine::test_common::TestProject;

// ---------------------------------------------------------------------------
// AC4.1 - VectorSortOrder: AssignTemp hoisting in array context
// ---------------------------------------------------------------------------

#[test]
fn vector_sort_order_a2a_produces_assign_temp() {
    // result[D] = vector_sort_order(vals[*], 1) must be hoisted into AssignTemp
    // because the whole-array sort cannot be done per-element.
    let project = TestProject::new("vso_a2a_struct")
        .indexed_dimension("D", 3)
        .array_with_ranges("vals[D]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_aux("result[D]", "vector_sort_order(vals[*], 1)");

    assert!(
        project.flow_runlist_has_assign_temp(),
        "VectorSortOrder in array context should produce AssignTemp in the flow runlist"
    );
}

#[test]
fn vector_sort_order_a2a_produces_correct_values_interpreter() {
    // vals = [30, 10, 20], ascending sort order (1)
    // sorted ascending: 10, 20, 30 -> original positions (1-based): 2, 3, 1
    // so result[D1]=2, result[D2]=3, result[D3]=1
    let project = TestProject::new("vso_a2a_vals_interp")
        .indexed_dimension("D", 3)
        .array_with_ranges("vals[D]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_aux("result[D]", "vector_sort_order(vals[*], 1)");

    project.assert_interpreter_result("result", &[2.0, 3.0, 1.0]);
}

#[test]
fn vector_sort_order_a2a_produces_correct_values_vm() {
    let project = TestProject::new("vso_a2a_vals_vm")
        .indexed_dimension("D", 3)
        .array_with_ranges("vals[D]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_aux("result[D]", "vector_sort_order(vals[*], 1)");

    project.assert_vm_result("result", &[2.0, 3.0, 1.0]);
}

// ---------------------------------------------------------------------------
// AC4.2 - VectorElmMap: AssignTemp hoisting in array context
// ---------------------------------------------------------------------------

#[test]
fn vector_elm_map_a2a_produces_assign_temp() {
    // result[D] = vector_elm_map(source[*], offsets[*]) must be hoisted into AssignTemp
    // because the whole-array mapping cannot be done per-element.
    let project = TestProject::new("vem_a2a_struct")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "0"), ("2", "2"), ("3", "1")])
        .array_aux("result[D]", "vector_elm_map(source[*], offsets[*])");

    assert!(
        project.flow_runlist_has_assign_temp(),
        "VectorElmMap in array context should produce AssignTemp in the flow runlist"
    );
}

#[test]
fn vector_elm_map_a2a_produces_correct_values_interpreter() {
    // source = [10, 20, 30], offsets = [0, 2, 1]
    // result: source[0]=10, source[2]=30, source[1]=20
    let project = TestProject::new("vem_a2a_vals_interp")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "0"), ("2", "2"), ("3", "1")])
        .array_aux("result[D]", "vector_elm_map(source[*], offsets[*])");

    project.assert_interpreter_result("result", &[10.0, 30.0, 20.0]);
}

#[test]
fn vector_elm_map_a2a_produces_correct_values_vm() {
    let project = TestProject::new("vem_a2a_vals_vm")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "0"), ("2", "2"), ("3", "1")])
        .array_aux("result[D]", "vector_elm_map(source[*], offsets[*])");

    project.assert_vm_result("result", &[10.0, 30.0, 20.0]);
}

#[test]
fn nested_vector_elm_map_inside_max_interpreter() {
    // source = [10, 20, 30], offsets = [2, 0, 1] => VEM = [30, 10, 20]
    // MAX(VEM, 15) should be element-wise: [30, 15, 20]
    let project = TestProject::new("vem_nested_max_interp")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .array_aux(
            "result[D]",
            "max(vector_elm_map(source[*], offsets[*]), 15)",
        );

    project.assert_interpreter_result("result", &[30.0, 15.0, 20.0]);
}

#[test]
fn nested_vector_elm_map_inside_max_vm() {
    let project = TestProject::new("vem_nested_max_vm")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .array_aux(
            "result[D]",
            "max(vector_elm_map(source[*], offsets[*]), 15)",
        );

    project.assert_vm_result("result", &[30.0, 15.0, 20.0]);
}

#[test]
fn scalar_max_with_vector_elm_map_returns_structured_vm_compile_error() {
    let project = TestProject::new("vem_scalar_max_vm_error")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .scalar_aux("result", "max(vector_elm_map(source[*], offsets[*]), 15)");

    let err = project
        .run_vm()
        .expect_err("scalar max(vector_elm_map(...), 15) should fail with a compile error");
    // The incremental path wraps the codegen error ("array-producing builtin
    // outside AssignTemp context") into a fragment assembly failure naming the
    // variable that couldn't be compiled.
    assert!(
        err.contains("failed to compile fragments for variables: result")
            || err.contains("array-producing builtin outside AssignTemp context"),
        "expected compile error mentioning 'result' or AssignTemp context, got: {err}"
    );
}

#[test]
fn nested_vector_elm_map_inside_sum_interpreter() {
    // source = [10, 20, 30], offsets = [2, 0, 1] => VEM = [30, 10, 20]
    // SUM(VEM) = 60
    let project = TestProject::new("vem_nested_sum_interp")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .scalar_aux("result", "sum(vector_elm_map(source[*], offsets[*]))");

    project.assert_interpreter_result("result", &[60.0, 60.0]);
}

#[test]
fn nested_vector_elm_map_inside_sum_vm() {
    let project = TestProject::new("vem_nested_sum_vm")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .scalar_aux("result", "sum(vector_elm_map(source[*], offsets[*]))");

    project.assert_vm_result("result", &[60.0, 60.0]);
}

#[test]
fn nested_vector_elm_map_inside_sum_in_array_context_interpreter() {
    // In array context, SUM(VEM(...)) should still evaluate VEM as an array,
    // not as an element-local scalar.
    let project = TestProject::new("vem_nested_sum_array_context_interp")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .array_aux("result[D]", "sum(vector_elm_map(source[*], offsets[*]))");

    project.assert_interpreter_result("result", &[60.0, 60.0, 60.0]);
}

#[test]
fn nested_vector_elm_map_inside_sum_in_array_context_vm() {
    let project = TestProject::new("vem_nested_sum_array_context_vm")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .array_aux("result[D]", "sum(vector_elm_map(source[*], offsets[*]))");

    project.assert_vm_result("result", &[60.0, 60.0, 60.0]);
}

#[test]
fn nested_vector_sort_order_inside_sum_in_array_context_interpreter() {
    // vals = [30, 10, 20], vector_sort_order(vals, 1) = [2, 3, 1], SUM = 6
    let project = TestProject::new("vso_nested_sum_array_context_interp")
        .indexed_dimension("D", 3)
        .array_with_ranges("vals[D]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_aux("result[D]", "sum(vector_sort_order(vals[*], 1))");

    project.assert_interpreter_result("result", &[6.0, 6.0, 6.0]);
}

#[test]
fn nested_vector_sort_order_inside_sum_in_array_context_vm() {
    let project = TestProject::new("vso_nested_sum_array_context_vm")
        .indexed_dimension("D", 3)
        .array_with_ranges("vals[D]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_aux("result[D]", "sum(vector_sort_order(vals[*], 1))");

    project.assert_vm_result("result", &[6.0, 6.0, 6.0]);
}

// ---------------------------------------------------------------------------
// AC4.3 - AllocateAvailable: AssignTemp hoisting in array context
// ---------------------------------------------------------------------------
//
// The pp (priority profile) array has layout [n_requesters, 4] where each row
// is [ptype, ppriority, pwidth, pextra]. ptype=3 selects the Normal distribution.
// With equal priority (ppriority=1) and supply=40, the allocation is proportional
// to request amounts.

fn make_alloc_project(name: &str) -> TestProject {
    // D: 3 requesters with requests [10, 20, 30]
    // XP: 4 priority-profile parameters (ptype, ppriority, pwidth, pextra)
    // pp[D,XP]: Normal dist (ptype=3), equal priority (ppriority=1), width=1, extra=0
    // With supply=40 and total requests=60, allocation is proportional: each
    // requester gets request * (40/60). Sum of all allocations = 40.
    TestProject::new(name)
        .indexed_dimension("D", 3)
        .indexed_dimension("XP", 4)
        .array_with_ranges("request[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .scalar_const("supply", 40.0)
        .array_with_ranges(
            "pp[D,XP]",
            vec![
                ("1,1", "3"),
                ("1,2", "1"),
                ("1,3", "1"),
                ("1,4", "0"),
                ("2,1", "3"),
                ("2,2", "1"),
                ("2,3", "1"),
                ("2,4", "0"),
                ("3,1", "3"),
                ("3,2", "1"),
                ("3,3", "1"),
                ("3,4", "0"),
            ],
        )
        .array_aux(
            "result[D]",
            "allocate_available(request[*], pp[*,1], supply)",
        )
        .scalar_aux("total_alloc", "result[1] + result[2] + result[3]")
}

#[test]
fn allocate_available_a2a_produces_assign_temp() {
    // result[D] = allocate_available(request[*], pp, supply) must be hoisted into
    // AssignTemp because the whole-array allocation cannot be done per-element.
    let project = make_alloc_project("alloc_a2a_struct");
    assert!(
        project.flow_runlist_has_assign_temp(),
        "AllocateAvailable in array context should produce AssignTemp in the flow runlist"
    );
}

#[test]
fn allocate_available_a2a_sums_to_supply_interpreter() {
    // With requests [10, 20, 30] and supply = 40, total allocated should equal supply.
    // Equal priority, Normal dist: all requests are within available supply, so each
    // requester gets their full request (10 + 20 + 30 = 60 > 40, so proportional).
    let project = make_alloc_project("alloc_a2a_sum_interp");
    project.assert_scalar_result("total_alloc", 40.0);
}

#[test]
fn allocate_available_a2a_sums_to_supply_vm() {
    let project = make_alloc_project("alloc_a2a_sum_vm");
    project.assert_scalar_result("total_alloc", 40.0);
}

#[test]
fn nested_allocate_available_inside_sum_in_array_context_interpreter() {
    // allocate_available(request, pp, supply) returns a D-array whose total is supply.
    // SUM(...) should therefore be 40, and in array context each element should
    // receive that same scalar reduction result.
    let project = TestProject::new("alloc_nested_sum_array_context_interp")
        .indexed_dimension("D", 3)
        .indexed_dimension("XP", 4)
        .array_with_ranges("request[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .scalar_const("supply", 40.0)
        .array_with_ranges(
            "pp[D,XP]",
            vec![
                ("1,1", "3"),
                ("1,2", "1"),
                ("1,3", "1"),
                ("1,4", "0"),
                ("2,1", "3"),
                ("2,2", "1"),
                ("2,3", "1"),
                ("2,4", "0"),
                ("3,1", "3"),
                ("3,2", "1"),
                ("3,3", "1"),
                ("3,4", "0"),
            ],
        )
        .array_aux(
            "result[D]",
            "sum(allocate_available(request[*], pp[*,1], supply))",
        );

    project.assert_interpreter_result("result", &[40.0, 40.0, 40.0]);
}

#[test]
fn nested_allocate_available_inside_sum_in_array_context_vm() {
    let project = TestProject::new("alloc_nested_sum_array_context_vm")
        .indexed_dimension("D", 3)
        .indexed_dimension("XP", 4)
        .array_with_ranges("request[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .scalar_const("supply", 40.0)
        .array_with_ranges(
            "pp[D,XP]",
            vec![
                ("1,1", "3"),
                ("1,2", "1"),
                ("1,3", "1"),
                ("1,4", "0"),
                ("2,1", "3"),
                ("2,2", "1"),
                ("2,3", "1"),
                ("2,4", "0"),
                ("3,1", "3"),
                ("3,2", "1"),
                ("3,3", "1"),
                ("3,4", "0"),
            ],
        )
        .array_aux(
            "result[D]",
            "sum(allocate_available(request[*], pp[*,1], supply))",
        );

    project.assert_vm_result("result", &[40.0, 40.0, 40.0]);
}

// ---------------------------------------------------------------------------
// AC5 - AllocateByPriority: native engine builtin
// ---------------------------------------------------------------------------
//
// ALLOCATE BY PRIORITY(request, priority, size, width, supply) desugars to
// ALLOCATE AVAILABLE at runtime by constructing rectangular (ptype=1) priority
// profiles from each requester's priority value and the shared width.
//
// Test scenario:
//   3 requesters with requests [10, 20, 30], priorities [3, 1, 2],
//   width=1, supply=35. Higher priority gets served first:
//     priority 3 (request=10): gets full 10
//     priority 2 (request=30): gets full 30 if supply allows, else remainder
//     priority 1 (request=20): gets remainder
//   With supply=35: requester 0 (pri=3) gets 10, requester 2 (pri=2) gets 25,
//   requester 1 (pri=1) gets 0. Total = 35.

fn make_alloc_by_priority_project(name: &str) -> TestProject {
    TestProject::new(name)
        .indexed_dimension("D", 3)
        .array_with_ranges("request[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("priority[D]", vec![("1", "3"), ("2", "1"), ("3", "2")])
        .scalar_const("supply", 35.0)
        .scalar_const("width", 1.0)
        .array_aux(
            "result[D]",
            "allocate_by_priority(request[*], priority[*], 0, width, supply)",
        )
        .scalar_aux("total_alloc", "result[1] + result[2] + result[3]")
}

#[test]
fn allocate_by_priority_compiles_and_runs_vm() {
    // AC5.1: allocate_by_priority in XMILE equations compiles and executes correctly
    let project = make_alloc_by_priority_project("alloc_by_pri_vm");
    // Total allocated should equal supply
    project.assert_scalar_result("total_alloc", 35.0);
}

#[test]
fn allocate_by_priority_compiles_and_runs_interpreter() {
    // AC5.1: same via interpreter
    let project = make_alloc_by_priority_project("alloc_by_pri_interp");
    project.assert_scalar_result("total_alloc", 35.0);
}

#[test]
fn allocate_by_priority_matches_allocate_available() {
    // AC5.2: Results must match ALLOCATE AVAILABLE with equivalent rectangular
    // priority profiles. For allocate_by_priority(req, priority, 0, width, supply),
    // the equivalent is allocate_available(req, pp, supply) where
    // pp[i] = (1, priority[i], width, 0).

    let by_priority = TestProject::new("alloc_equiv_by_pri")
        .indexed_dimension("D", 3)
        .array_with_ranges("request[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("priority[D]", vec![("1", "3"), ("2", "1"), ("3", "2")])
        .scalar_const("supply", 35.0)
        .scalar_const("width", 1.0)
        .array_aux(
            "result[D]",
            "allocate_by_priority(request[*], priority[*], 0, width, supply)",
        );

    // Equivalent using allocate_available with explicit rectangular profiles
    let by_available = TestProject::new("alloc_equiv_avail")
        .indexed_dimension("D", 3)
        .indexed_dimension("XP", 4)
        .array_with_ranges("request[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .scalar_const("supply", 35.0)
        .array_with_ranges(
            "pp[D,XP]",
            vec![
                // Requester 1: ptype=1, ppriority=3, pwidth=1, pextra=0
                ("1,1", "1"),
                ("1,2", "3"),
                ("1,3", "1"),
                ("1,4", "0"),
                // Requester 2: ptype=1, ppriority=1, pwidth=1, pextra=0
                ("2,1", "1"),
                ("2,2", "1"),
                ("2,3", "1"),
                ("2,4", "0"),
                // Requester 3: ptype=1, ppriority=2, pwidth=1, pextra=0
                ("3,1", "1"),
                ("3,2", "2"),
                ("3,3", "1"),
                ("3,4", "0"),
            ],
        )
        .array_aux(
            "result[D]",
            "allocate_available(request[*], pp[*,1], supply)",
        );

    let pri_results = by_priority
        .run_vm()
        .expect("allocate_by_priority VM should succeed");
    let avail_results = by_available
        .run_vm()
        .expect("allocate_available VM should succeed");

    let pri_vals = pri_results
        .get("result")
        .expect("result should exist in by_priority");
    let avail_vals = avail_results
        .get("result")
        .expect("result should exist in by_available");

    assert_eq!(
        pri_vals.len(),
        avail_vals.len(),
        "result array lengths should match"
    );
    for (i, (p, a)) in pri_vals.iter().zip(avail_vals.iter()).enumerate() {
        assert!(
            (p - a).abs() < 1e-6,
            "result[{i}] mismatch: allocate_by_priority={p}, allocate_available={a}"
        );
    }
}
