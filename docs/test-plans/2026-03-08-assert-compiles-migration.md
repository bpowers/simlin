# Test Plan: Assert-Compiles Migration

## Prerequisites
- Engine builds cleanly: `cargo build -p simlin-engine`
- All engine tests pass: `cargo test -p simlin-engine`
- Full integration suite passes: `cargo test --features file_io --test simulate`

## Phase 1: @N Position Syntax

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Run `cargo test -p simlin-engine dimension_position_single -- --exact` | Test passes, confirming `arr[@1]` resolves to first element (10.0) in scalar context |
| 1.2 | Run `cargo test -p simlin-engine dimension_position_and_wildcard -- --exact` | Test passes, confirming `cube[@1, *, @3]` compiles on incremental path |
| 1.3 | Run `cargo test -p simlin-engine dimension_position_zero_is_error -- --exact` | Test passes, confirming `@0` is rejected with an error referencing the variable name |
| 1.4 | Run `cargo test -p simlin-engine dimension_position_out_of_range_is_error -- --exact` | Test passes, confirming `@5` on a size-3 dimension is rejected |
| 1.5 | Run `cargo test -p simlin-engine dimension_position_reorder -- --exact` | Regression guard passes, confirming A2A `matrix[@2, @1]` transpose still works |
| 1.6 | Run `cargo test -p simlin-engine dimension_position_3d -- --exact` | Regression guard passes, confirming 3D A2A `cube[@3, @2, @1]` reorder still works |

## Phase 2: MEAN with Dynamic Ranges

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Run `cargo test -p simlin-engine mean_with_dynamic_range -- --exact` | Test passes, result = 25.0 (correct MEAN of 2-element subrange) |
| 2.2 | Run `cargo test -p simlin-engine sum_with_dynamic_range -- --exact` | Regression guard: SUM with dynamic range still works |
| 2.3 | Run `cargo test -p simlin-engine size_with_dynamic_range -- --exact` | Regression guard: SIZE with dynamic range returns actual range size |
| 2.4 | Run `cargo test -p simlin-engine stddev_with_dynamic_range -- --exact` | Regression guard: STDDEV with dynamic range uses correct count |
| 2.5 | Run `cargo test -p simlin-engine size_with_empty_dynamic_range -- --exact` | Regression guard: SIZE with reversed range returns 0 |

## Phase 3: assert_compiles Removal

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | Run `cargo test -p simlin-engine` (full engine test suite) | All tests pass with zero failures |
| 3.2 | Search for old method: `grep -rn 'fn assert_compiles\b' src/simlin-engine/ \| grep -v incremental` | Zero matches |
| 3.3 | Search for old call sites: `grep -rn '\.assert_compiles(' src/simlin-engine/ \| grep -v incremental` | Zero matches |
| 3.4 | Verify `compile()` has callers: `grep -rn '\.compile()' src/simlin-engine/src/test_common.rs src/simlin-engine/tests/simulate_ltm.rs` | Active callers in `test_common.rs:588` and 10 sites in `simulate_ltm.rs` |

## End-to-End Regression Suite

| Step | Action | Expected |
|------|--------|----------|
| E1 | Run `cargo test -p simlin-engine` | All engine tests pass |
| E2 | Run `cargo test --features file_io --test simulate` | All simulation integration tests pass |
| E3 | Run `cargo test --features file_io --test simulate_ltm` | All LTM simulation tests pass |
| E4 | Run `cargo build -p simlin-engine --release` | Release build succeeds |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 `arr[@1]` scalar context | `dimension_position_single` | 1.1 |
| AC1.2 `cube[@1, *, @3]` mixed | `dimension_position_and_wildcard` | 1.2 |
| AC1.3 `arr[@0]` error | `dimension_position_zero_is_error` | 1.3 |
| AC1.4 `arr[@N]` out-of-range error | `dimension_position_out_of_range_is_error` | 1.4 |
| AC2.1 MEAN with variable bounds | `mean_with_dynamic_range` | 2.1 |
| AC2.2 Existing builtins unchanged | Existing array-reduce suite (50+ tests) | 2.2-2.5 |
| AC3.1 All 26 tests pass incremental | 20 builtins_visitor + 2 db_tests + 1 db_prev_init + 3 new | 3.1 |
| AC3.2 `assert_compiles()` deleted | Build succeeds | 3.2, 3.3 |
| AC3.3 `compile()` retained | -- | 3.4 |
| AC3.4 Zero regressions | `cargo test -p simlin-engine` | E1-E4 |
