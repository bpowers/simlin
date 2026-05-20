// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `Vm::set_value` / constant-override tests.
//!
//! Split out of `vm.rs` as a `#[path]`-included child module (so `super::*`
//! still reaches the parent module internals) to keep `vm.rs` under the
//! per-file line cap. Behaviour is unchanged; this is a pure relocation.

use super::*;
use crate::test_common::TestProject;

/// Model: rate=0.1, scaled_rate=rate*10, stock initial=scaled_rate.
/// `rate` and `scaled_rate` are both stock dependencies in the initials.
fn rate_model() -> TestProject {
    TestProject::new("rate_model")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("rate", "0.1", None)
        .aux("scaled_rate", "rate * 10", None)
        .flow("inflow", "population * rate", None)
        .flow("outflow", "population / 80", None)
        .stock("population", "scaled_rate", &["inflow"], &["outflow"], None)
}

fn build_compiled(tp: &TestProject) -> CompiledSimulation {
    tp.compile_incremental()
        .expect("incremental compile should succeed")
}

#[test]
fn test_override_constant_flows_through_dependent_initials() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();

    // Override rate from 0.1 to 0.2
    vm.set_value(&Ident::new("rate"), 0.2).unwrap();
    vm.run_initials().unwrap();

    let rate_off = vm.get_offset(&Ident::new("rate")).unwrap();
    let sr_off = vm.get_offset(&Ident::new("scaled_rate")).unwrap();
    let pop_off = vm.get_offset(&Ident::new("population")).unwrap();

    assert_eq!(
        vm.get_value_now(rate_off),
        0.2,
        "rate should be overridden to 0.2"
    );
    assert_eq!(
        vm.get_value_now(sr_off),
        2.0,
        "scaled_rate = rate*10 = 0.2*10 = 2.0"
    );
    assert_eq!(
        vm.get_value_now(pop_off),
        2.0,
        "population initial = scaled_rate = 2.0"
    );
}

#[test]
fn test_override_affects_simulation_results() {
    let compiled = build_compiled(&rate_model());

    // Run without override
    let mut vm1 = Vm::new(compiled.clone()).unwrap();
    vm1.run_to_end().unwrap();
    let series1 = vm1.get_series(&Ident::new("population")).unwrap();

    // Run with override: higher rate means more growth
    let mut vm2 = Vm::new(compiled).unwrap();
    vm2.set_value(&Ident::new("rate"), 0.2).unwrap();
    vm2.run_to_end().unwrap();
    let series2 = vm2.get_series(&Ident::new("population")).unwrap();

    assert!(
        series2.last().unwrap() > series1.last().unwrap(),
        "higher rate should produce higher final population: {} vs {}",
        series2.last().unwrap(),
        series1.last().unwrap()
    );

    // Verify the override affects flows: rate should be 0.2 throughout
    let rate_series = vm2.get_series(&Ident::new("rate")).unwrap();
    for (i, &val) in rate_series.iter().enumerate() {
        assert!(
            (val - 0.2).abs() < 1e-10,
            "rate should be 0.2 at every step, got {} at step {}",
            val,
            i
        );
    }
}

#[test]
fn test_override_persists_across_reset() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();

    vm.set_value(&Ident::new("rate"), 0.2).unwrap();
    vm.run_to_end().unwrap();
    let series_before = vm.get_series(&Ident::new("population")).unwrap();

    vm.reset();
    vm.run_to_end().unwrap();
    let series_after = vm.get_series(&Ident::new("population")).unwrap();

    for (i, (a, b)) in series_before.iter().zip(series_after.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-10,
            "override should persist across reset: step {i}: {a} vs {b}"
        );
    }
}

#[test]
fn test_clear_values_restores_defaults() {
    let compiled = build_compiled(&rate_model());

    // Baseline run
    let mut vm_baseline = Vm::new(compiled.clone()).unwrap();
    vm_baseline.run_to_end().unwrap();
    let baseline = vm_baseline.get_series(&Ident::new("population")).unwrap();

    // Run with override
    let mut vm = Vm::new(compiled).unwrap();
    vm.set_value(&Ident::new("rate"), 0.5).unwrap();
    vm.run_to_end().unwrap();
    let overridden = vm.get_series(&Ident::new("population")).unwrap();

    // Clear and re-run
    vm.clear_values();
    vm.reset();
    vm.run_to_end().unwrap();
    let restored = vm.get_series(&Ident::new("population")).unwrap();

    // Overridden should differ from baseline
    assert!(
        (overridden.last().unwrap() - baseline.last().unwrap()).abs() > 1.0,
        "overridden should differ from baseline"
    );
    // Restored should match baseline
    for (i, (b, r)) in baseline.iter().zip(restored.iter()).enumerate() {
        assert!(
            (b - r).abs() < 1e-10,
            "after clear_values, should match baseline: step {i}: {b} vs {r}"
        );
    }
}

#[test]
fn test_multiple_reset_set_value_cycles() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();

    let mut prev_final = 0.0;
    for i in 1..=10 {
        let rate_val = i as f64 * 0.01;
        vm.set_value(&Ident::new("rate"), rate_val).unwrap();
        vm.reset();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&Ident::new("population")).unwrap();
        let final_val = *series.last().unwrap();
        if i > 1 {
            assert!(
                final_val > prev_final,
                "final pop should increase with rate: rate={rate_val}, final={final_val}, prev={prev_final}"
            );
        }
        prev_final = final_val;
    }
}

#[test]
fn test_override_nonexistent_variable_returns_error() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();
    let result = vm.set_value(&Ident::new("nonexistent_var"), 1.0);
    assert!(
        result.is_err(),
        "overriding nonexistent variable should fail"
    );
}

#[test]
fn test_set_value_returns_correct_offset() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();
    let rate_ident = Ident::new("rate");

    let expected_off = vm.get_offset(&rate_ident).unwrap();
    let returned_off = vm.set_value(&rate_ident, 0.5).unwrap();
    assert_eq!(
        returned_off, expected_off,
        "set_value should return the data-buffer offset of the variable"
    );
}

#[test]
fn test_override_by_offset_out_of_bounds_returns_error() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();
    let err = vm.set_value_by_offset(99999, 1.0).unwrap_err();
    assert_eq!(err.code, crate::common::ErrorCode::BadOverride);
}

#[test]
fn test_set_value_non_constant_variable_returns_error() {
    // `births = pop * birth_rate` is a computed flow, not a simple constant
    let tp = TestProject::new("non_constant_override")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("birth_rate", "0.1", None)
        .flow("births", "pop * birth_rate", None)
        .stock("pop", "100", &["births"], &[], None);

    let compiled = tp.compile_incremental().unwrap();
    let mut vm = Vm::new(compiled).unwrap();

    // birth_rate IS a simple constant, so set_value should succeed
    vm.set_value(&Ident::new("birth_rate"), 0.5).unwrap();

    // births is a computed flow (not a constant), so set_value should fail
    let err = vm.set_value(&Ident::new("births"), 42.0).unwrap_err();
    assert_eq!(err.code, crate::common::ErrorCode::BadOverride);
}

#[test]
fn test_set_value_non_constant_returns_error() {
    let tp = TestProject::new("non_constant_set")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("rate", "0.1", None)
        .aux("computed", "rate * 10", None)
        .flow("inflow", "pop * rate", None)
        .stock("pop", "100", &["inflow"], &[], None);

    let compiled = tp.compile_incremental().unwrap();
    let mut vm = Vm::new(compiled).unwrap();

    // "computed" depends on "rate", so it's not a simple constant
    let err = vm.set_value(&Ident::new("computed"), 5.0).unwrap_err();
    assert_eq!(err.code, crate::common::ErrorCode::BadOverride);

    // Stocks also cannot be set via set_value
    let err = vm.set_value(&Ident::new("pop"), 500.0).unwrap_err();
    assert_eq!(err.code, crate::common::ErrorCode::BadOverride);
}

#[test]
fn test_set_value_after_initials_affects_flows_but_not_stock_initials() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();

    // Run initials first (stock initial = scaled_rate = rate*10 = 1.0)
    vm.run_initials().unwrap();

    // Set value AFTER initials
    vm.set_value(&Ident::new("rate"), 0.5).unwrap();

    // The stock initial is already set (from rate=0.1), but flows will use rate=0.5
    vm.run_to_end().unwrap();
    let series1 = vm.get_series(&Ident::new("population")).unwrap();

    // Now reset and run - BOTH initials and flows use rate=0.5
    vm.reset();
    vm.run_to_end().unwrap();
    let series2 = vm.get_series(&Ident::new("population")).unwrap();

    // series1 used rate=0.1 for initials but rate=0.5 for flows
    // series2 used rate=0.5 for both
    // They should differ (different initial stock values)
    assert!(
        (series1[0] - series2[0]).abs() > 0.1,
        "initial stock values should differ: first={}, second={}",
        series1[0],
        series2[0]
    );
}

#[test]
fn test_conflicting_writes_to_same_offset() {
    let compiled = build_compiled(&rate_model());
    let mut vm = Vm::new(compiled).unwrap();

    let rate_off = vm.get_offset(&Ident::new("rate")).unwrap();

    // Two writes to the same offset - last one wins
    vm.set_value_by_offset(rate_off, 0.1).unwrap();
    vm.set_value_by_offset(rate_off, 0.3).unwrap();

    vm.run_initials().unwrap();
    assert_eq!(vm.get_value_now(rate_off), 0.3, "last override should win");
}

#[test]
fn test_set_value_module_stock_returns_error() {
    let test_file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx"
    );
    let file_bytes =
        std::fs::read(test_file).expect("modules_hares_and_foxes test fixture must exist");
    let mut cursor = std::io::Cursor::new(file_bytes);
    let project_datamodel = crate::open_xmile(&mut cursor).unwrap();
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project_datamodel, None);
    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main").unwrap();

    let mut vm = Vm::new(compiled).unwrap();
    let hares_ident = Ident::new("hares.hares");
    assert!(
        vm.get_offset(&hares_ident).is_some(),
        "hares·hares should exist in offsets"
    );
    // Stocks are not simple constants, so set_value should fail
    let err = vm.set_value(&hares_ident, 500.0).unwrap_err();
    assert_eq!(err.code, crate::common::ErrorCode::BadOverride);
}

#[test]
fn test_override_partial_array() {
    let tp = TestProject::new("array_override")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Dim", &["A", "B", "C"])
        .array_with_ranges("arr[Dim]", vec![("A", "1"), ("B", "2"), ("C", "3")])
        .aux("total", "arr[A] + arr[B] + arr[C]", None)
        .flow("inflow", "0", None)
        .stock("s", "total", &["inflow"], &[], None);

    let compiled = tp.compile_incremental().unwrap();
    let mut vm = Vm::new(compiled).unwrap();

    let arr_b_ident = Ident::new("arr[b]");
    let arr_b_off = vm
        .get_offset(&arr_b_ident)
        .expect("arr[b] should exist in offsets");
    vm.set_value_by_offset(arr_b_off, 99.0).unwrap();
    vm.run_initials().unwrap();
    assert_eq!(
        vm.get_value_now(arr_b_off),
        99.0,
        "arr[b] should be overridden to 99"
    );
    let s_off = vm.get_offset(&Ident::new("s")).unwrap();
    // total = arr[A]+arr[B]+arr[C] = 1+99+3 = 103
    assert_eq!(
        vm.get_value_now(s_off),
        103.0,
        "stock should reflect overridden array element: 1+99+3=103"
    );
}

#[test]
fn test_set_value_affects_flow_computation() {
    // Model where birth_rate is ONLY used in flows (not in stock initial)
    let tp = TestProject::new("flow_only_constant")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("birth_rate", "0.1", None)
        .flow("births", "pop * birth_rate", None)
        .stock("pop", "100", &["births"], &[], None);

    let compiled = tp.compile_incremental().unwrap();

    // Run without override
    let mut vm1 = Vm::new(compiled.clone()).unwrap();
    vm1.run_to_end().unwrap();
    let series1 = vm1.get_series(&Ident::new("pop")).unwrap();

    // Run with override
    let mut vm2 = Vm::new(compiled).unwrap();
    vm2.set_value(&Ident::new("birth_rate"), 0.5).unwrap();
    vm2.run_to_end().unwrap();
    let series2 = vm2.get_series(&Ident::new("pop")).unwrap();

    // Higher birth_rate should produce higher final population
    assert!(
        series2.last().unwrap() > series1.last().unwrap(),
        "higher birth_rate should produce higher final population: {} vs {}",
        series2.last().unwrap(),
        series1.last().unwrap()
    );

    // Verify birth_rate shows the overridden value
    let br_series = vm2.get_series(&Ident::new("birth_rate")).unwrap();
    for (i, &val) in br_series.iter().enumerate() {
        assert!(
            (val - 0.5).abs() < 1e-10,
            "birth_rate should be 0.5 at step {}, got {}",
            i,
            val
        );
    }
}

#[test]
fn test_override_does_not_corrupt_shared_literal() {
    // Two constants with the same numeric value used to share an interned
    // literal_id. Now they get distinct slots via push_named_literal.
    // Overriding one must NOT affect the other.
    let tp = TestProject::new("shared_literal")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("rate_a", "0.1", None)
        .aux("rate_b", "0.1", None)
        .aux("total_rate", "rate_a + rate_b", None)
        .flow("inflow", "stock_val * total_rate", None)
        .stock("stock_val", "100", &["inflow"], &[], None);

    let compiled = tp.compile_incremental().unwrap();
    let mut vm = Vm::new(compiled).unwrap();

    // Override rate_a only, then run the full simulation
    vm.set_value(&Ident::new("rate_a"), 0.5).unwrap();
    vm.run_to_end().unwrap();

    let rate_a_series = vm.get_series(&Ident::new("rate_a")).unwrap();
    let rate_b_series = vm.get_series(&Ident::new("rate_b")).unwrap();

    for (i, &val) in rate_a_series.iter().enumerate() {
        assert!(
            (val - 0.5).abs() < 1e-10,
            "rate_a should be 0.5 at step {i}, got {val}"
        );
    }
    for (i, &val) in rate_b_series.iter().enumerate() {
        assert!(
            (val - 0.1).abs() < 1e-10,
            "rate_b should remain 0.1 at step {i}, got {val} (must not be corrupted by rate_a override)"
        );
    }
}

#[test]
fn test_override_does_not_corrupt_expression_literal() {
    // A constant and an expression both use the same numeric value 0.1.
    // Overriding the constant must not corrupt the expression's literal.
    let tp = TestProject::new("expr_literal")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("rate", "0.1", None)
        .aux("scaled", "stock_val * 0.1", None)
        .flow("inflow", "stock_val * rate + scaled", None)
        .stock("stock_val", "100", &["inflow"], &[], None);

    let compiled = tp.compile_incremental().unwrap();
    let mut vm = Vm::new(compiled).unwrap();

    vm.set_value(&Ident::new("rate"), 0.9).unwrap();
    vm.run_to_end().unwrap();

    let scaled_series = vm.get_series(&Ident::new("scaled")).unwrap();
    // At t=0, stock_val = 100, so scaled = 100 * 0.1 = 10.0
    // If the literal 0.1 was corrupted to 0.9, scaled would be 90.0
    assert!(
        (scaled_series[0] - 10.0).abs() < 1e-10,
        "scaled should be 10.0 at t=0 (the 0.1 literal in the expression must not be corrupted), got {}",
        scaled_series[0]
    );
}

#[test]
fn test_same_valued_constants_get_distinct_literal_ids() {
    // Two constants with the same numeric value should get distinct literal
    // slots in their AssignConstCurr opcodes (via push_named_literal).
    let tp = TestProject::new("distinct_lits")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("rate_a", "0.1", None)
        .aux("rate_b", "0.1", None)
        .flow("inflow", "rate_a + rate_b", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = tp.compile_incremental().unwrap();
    let root_module = &compiled.modules[&compiled.root];

    // Collect all AssignConstCurr literal_ids from the flows bytecode.
    let assign_const_lits: Vec<u16> = root_module
        .compiled_flows
        .code
        .iter()
        .filter_map(|op| {
            if let Opcode::AssignConstCurr { literal_id, .. } = op {
                Some(*literal_id)
            } else {
                None
            }
        })
        .collect();

    // rate_a and rate_b each get their own AssignConstCurr with distinct literal_ids.
    assert!(
        assign_const_lits.len() >= 2,
        "expected at least 2 AssignConstCurr opcodes, got {}",
        assign_const_lits.len()
    );
    // All literal_ids should be unique (no sharing).
    let unique: std::collections::HashSet<u16> = assign_const_lits.iter().copied().collect();
    assert_eq!(
        unique.len(),
        assign_const_lits.len(),
        "literal_ids should all be distinct, got {:?}",
        assign_const_lits
    );
}

#[test]
fn test_override_shared_literal_clear_restores_both() {
    let tp = TestProject::new("shared_clear")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("rate_a", "0.1", None)
        .aux("rate_b", "0.1", None)
        .flow("inflow", "rate_a + rate_b", None)
        .stock("s", "rate_a + rate_b", &["inflow"], &[], None);

    let compiled = tp.compile_incremental().unwrap();
    let mut vm = Vm::new(compiled).unwrap();

    vm.set_value(&Ident::new("rate_a"), 0.5).unwrap();
    vm.run_to_end().unwrap();

    let rate_a_series = vm.get_series(&Ident::new("rate_a")).unwrap();
    let rate_b_series = vm.get_series(&Ident::new("rate_b")).unwrap();
    assert!(
        (rate_a_series[0] - 0.5).abs() < 1e-10,
        "rate_a should be 0.5"
    );
    assert!(
        (rate_b_series[0] - 0.1).abs() < 1e-10,
        "rate_b should be 0.1"
    );

    // Clear and re-run
    vm.clear_values();
    vm.reset();
    vm.run_to_end().unwrap();

    let rate_a_restored = vm.get_series(&Ident::new("rate_a")).unwrap();
    let rate_b_restored = vm.get_series(&Ident::new("rate_b")).unwrap();
    assert!(
        (rate_a_restored[0] - 0.1).abs() < 1e-10,
        "rate_a should be restored to 0.1, got {}",
        rate_a_restored[0]
    );
    assert!(
        (rate_b_restored[0] - 0.1).abs() < 1e-10,
        "rate_b should still be 0.1, got {}",
        rate_b_restored[0]
    );
}
