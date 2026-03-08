# Test Requirements: Assert-Compiles Migration

This document maps every acceptance criterion from the [design plan](../../design-plans/2026-03-07-assert-compiles-migration.md) to either an automated test or a documented human verification step.

---

## Automated Tests

### AC1: @N position syntax works on incremental path

| Criterion | Test Type | Test Name | Test File | Phase/Task |
|-----------|-----------|-----------|-----------|------------|
| AC1.1 `arr[@1]` scalar context | Unit (compile + simulate) | `dimension_position_single` | `src/simlin-engine/src/array_tests.rs` | P1/T2 |
| AC1.2 `cube[@1, *, @3]` mixed | Unit (compile) | `dimension_position_and_wildcard` | `src/simlin-engine/src/array_tests.rs` | P1/T2 |
| AC1.3 `arr[@0]` error | Unit (compile error) | `dimension_position_zero_is_error` (new) | `src/simlin-engine/src/array_tests.rs` | P1/T3 |
| AC1.4 `arr[@N]` out-of-range error | Unit (compile error) | `dimension_position_out_of_range_is_error` (new) | `src/simlin-engine/src/array_tests.rs` | P1/T3 |

Regression guard: existing `dimension_position_reorder` and `dimension_position_3d` confirm A2A path is unaffected.

### AC2: MEAN with dynamic ranges works on incremental path

| Criterion | Test Type | Test Name | Test File | Phase/Task |
|-----------|-----------|-----------|-----------|------------|
| AC2.1 MEAN with variable bounds | Unit (compile + simulate) | `mean_with_dynamic_range` | `src/simlin-engine/src/array_tests.rs` | P2/T3 |
| AC2.2 Existing builtins unchanged | Regression | All existing array-reduce tests | `src/simlin-engine/src/array_tests.rs` | P2/T1 |

AC2.2 rationale: `emit_array_reduce` is a mechanical extraction of duplicated code. Each builtin already has test coverage. P2/T1 is a separate commit from P2/T2 so `cargo test` at the boundary gates regressions.

### AC3: assert_compiles fully removed

| Criterion | Test Type | Test Name(s) | Phase/Task |
|-----------|-----------|-------------|------------|
| AC3.1 All 26 tests pass incremental | Unit (compile) | 20 in `builtins_visitor`, 2 in `db_tests`, 1 in `db_prev_init_tests`, plus 3 from P1/P2 | P3/T1-T2 |
| AC3.2 `assert_compiles()` deleted | Build (compilation) | N/A -- absence verified by successful build | P3/T3 |
| AC3.3 `compile()` retained | Human verification | See below | P3/T3 |
| AC3.4 Zero regressions | Full suite | `cargo test -p simlin-engine` | P3/T3 |

---

## Human Verification

### AC3.3: `compile()` method retention

**Criterion:** `compile()` method deleted from `test_common.rs` (if no other callers).

**Decision:** `compile()` is NOT deleted -- it has active callers:
1. `assert_compile_error_impl()` at `test_common.rs:603`
2. `tests/simulate_ltm.rs` -- 10 call sites

**Verification:** After deleting `assert_compiles()`, grep for `.compile()` in engine test files to confirm callers remain. If they do (expected), `compile()` stays and AC3.3 is satisfied.

---

## Traceability Matrix

| AC | Automated Test(s) | Human | Phase |
|----|-------------------|-------|-------|
| AC1.1 | `dimension_position_single` | -- | P1 |
| AC1.2 | `dimension_position_and_wildcard` | -- | P1 |
| AC1.3 | `dimension_position_zero_is_error` (new) | -- | P1 |
| AC1.4 | `dimension_position_out_of_range_is_error` (new) | -- | P1 |
| AC2.1 | `mean_with_dynamic_range` | -- | P2 |
| AC2.2 | Existing array-reduce suite (regression) | -- | P2 |
| AC3.1 | 26 switched tests | -- | P1+P2+P3 |
| AC3.2 | Build succeeds after deletion | -- | P3 |
| AC3.3 | -- | Verify `compile()` callers | P3 |
| AC3.4 | `cargo test -p simlin-engine` | -- | P3 |
