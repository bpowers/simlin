# Assert-Compiles Migration -- Phase 3: Bulk Test Migration and Cleanup

**Goal:** Switch all remaining 23 tests from `assert_compiles()` to `assert_compiles_incremental()`, then delete `assert_compiles()`.

**Architecture:** Mechanical replacement of `assert_compiles()` with `assert_compiles_incremental()` across 3 files (20 + 2 + 1 calls), followed by deleting the `assert_compiles()` method from `test_common.rs`. The `compile()` method is NOT deleted because it has other active callers (`assert_compile_error_impl` and `tests/simulate_ltm.rs`).

**Tech Stack:** Rust (simlin-engine)

**Scope:** Phase 3 of 3 from design plan

**Codebase verified:** 2026-03-08

**Reference files for executor:**
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` -- engine architecture and module map
- `/home/bpowers/src/simlin/CLAUDE.md` -- project-wide development standards

---

## Acceptance Criteria Coverage

This phase implements and tests:

### assert-compiles-migration.AC3: assert_compiles fully removed
- **assert-compiles-migration.AC3.1 Success:** All 26 formerly-monolithic tests pass with `assert_compiles_incremental()`
- **assert-compiles-migration.AC3.2 Success:** `assert_compiles()` method deleted from `test_common.rs`
- **assert-compiles-migration.AC3.3 Success:** `compile()` method deleted from `test_common.rs` (if no other callers)
- **assert-compiles-migration.AC3.4 Success:** `cargo test -p simlin-engine` passes with zero regressions

---

## Codebase Verification Findings

- **Confirmed:** 20 `assert_compiles()` calls in `builtins_visitor.rs` at lines: 741, 756, 779, 803, 827, 846, 860, 874, 887, 901, 928, 942, 965, 982, 1004, 1027, 1053, 1074, 1086, 1100.
- **Confirmed:** 2 calls in `db_tests.rs` at lines 4974 and 5062.
- **Confirmed:** 1 call in `db_prev_init_tests.rs` at line 81.
- **Important: `compile()` method (lines 387-433) cannot be deleted.** It has other active callers:
  - `assert_compile_error_impl()` at line 603 (called by `assert_compile_error()` which has active callers throughout `array_tests.rs` and `builtins_visitor.rs`)
  - `tests/simulate_ltm.rs` at 10 call sites (lines 582, 609, 656, 685, 742, 783, 824, 888, 933, 1003)
- **AC3.3 clarification:** `compile()` has other callers, so it stays. AC3.3 is satisfied with the note "other callers remain."

---

<!-- START_TASK_1 -->
### Task 1: Switch `builtins_visitor.rs` tests to incremental path

**Verifies:** assert-compiles-migration.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs`

**Implementation:**

Replace `assert_compiles()` with `assert_compiles_incremental()` in all 20 test functions listed below. This is a mechanical find-and-replace within the `#[cfg(test)]` module. Remove any comments about the monolithic path or incremental support gaps if present.

Test functions (line numbers for the `assert_compiles()` call):
- `test_arrayed_delay1_basic` (741)
- `test_arrayed_delay1_mixed_args` (756)
- `test_arrayed_delay1_numerical_values` (779)
- `test_arrayed_delay1_all_arrayed` (803)
- `test_arrayed_delay1_different_element_values` (827)
- `test_arrayed_delay3` (846)
- `test_arrayed_delayn_order1` (860)
- `test_arrayed_delayn_order3` (874)
- `test_arrayed_smooth1` (887)
- `test_arrayed_smthn_order1` (901)
- `test_arrayed_delay1_indexed_dimension` (928)
- `test_arrayed_delay_in_expression` (942)
- `test_arrayed_per_element_delay1` (965)
- `test_arrayed_per_element_mixed_stdlib` (982)
- `test_arrayed_per_element_delay1_with_subscripted_inputs` (1004)
- `test_npv_basic` (1027)
- `test_npv_with_discount` (1053)
- `test_modulo_function` (1074)
- `test_nested_init_does_not_rewrite_generated_arg_helpers` (1086)
- `test_delay_alias` (1100)

**Verification:**
Run: `cargo test -p simlin-engine builtins_visitor`
Expected: All 20 tests pass.

**Commit:** `engine: switch builtins_visitor tests to incremental path`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Switch `db_tests.rs` and `db_prev_init_tests.rs` tests to incremental path

**Verifies:** assert-compiles-migration.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs:4974` (`test_arrayed_1arg_previous_loadprev_per_element`)
- Modify: `src/simlin-engine/src/db_tests.rs:5062` (`test_arrayed_2arg_previous_per_element_modules`)
- Modify: `src/simlin-engine/src/db_prev_init_tests.rs:81` (`test_nested_previous_does_not_create_false_cycle_via_helper_deps`)

**Implementation:**

Replace `assert_compiles()` with `assert_compiles_incremental()` in all 3 test functions. Remove any comments about the monolithic path if present.

**Verification:**
Run: `cargo test -p simlin-engine test_arrayed_1arg_previous`
Run: `cargo test -p simlin-engine test_arrayed_2arg_previous`
Run: `cargo test -p simlin-engine test_nested_previous_does_not_create_false_cycle`
Expected: All 3 pass.

**Commit:** `engine: switch db_tests and db_prev_init_tests to incremental path`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Delete `assert_compiles()` method from `test_common.rs`

**Verifies:** assert-compiles-migration.AC3.2, assert-compiles-migration.AC3.4

**Files:**
- Modify: `src/simlin-engine/src/test_common.rs:532-545` (delete `assert_compiles()` method)

**Implementation:**

Delete the `assert_compiles()` method (lines 532-545). Do NOT delete `compile()` (lines 387-433) -- it has other active callers (`assert_compile_error_impl` and `tests/simulate_ltm.rs`).

Verify there are zero remaining calls to `assert_compiles()` (not `assert_compiles_incremental`) in the engine codebase. The grep should return zero results.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All engine tests pass. No compilation errors.

**Commit:** `engine: delete assert_compiles() method`
<!-- END_TASK_3 -->
