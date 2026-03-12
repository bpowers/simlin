# Close Array Gaps Implementation Plan -- Phase 6

**Goal:** Enable the 7 previously-ignored `array_tests.rs` unit tests, verify and close the 2 resolved issues, and clean up dead code suppressions.

**Architecture:** Several independent fixes: (1) compiler support for dimension-mismatched A2A assignment with NaN fill for out-of-bounds elements; (2) dynamic range bounds in A2A context; (3) transpose + subscript chaining; (4) adding `parent` field to `Dimension` for indexed subdimension relationships; (5) documentation comments for the 0-based/1-based indexing asymmetry; (6) dead code cleanup and tech-debt updates.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 6 phases from original design (this is phase 6 of 6)

**Codebase verified:** 2026-03-11

**Testing references:** See `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` for test index.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### close-array-gaps.AC9: Ignored array_tests.rs tests enabled
- **close-array-gaps.AC9.1 Success:** `range_basic` passes with NaN fill for out-of-bounds elements
- **close-array-gaps.AC9.2 Success:** `range_with_expressions` passes with dynamic range bounds in A2A context
- **close-array-gaps.AC9.3 Success:** `out_of_bounds_iteration_returns_nan` passes
- **close-array-gaps.AC9.4 Success:** `bounds_check_in_fast_path` passes
- **close-array-gaps.AC9.5 Success:** `transpose_and_slice` passes
- **close-array-gaps.AC9.6 Success:** `complex_expression` compiles (parser accepts `Dimension.*` subscript syntax)
- **close-array-gaps.AC9.7 Success:** `star_to_indexed_subdimension` passes (datamodel has parent pointer for indexed subdimensions)

### close-array-gaps.AC10: Verify and cleanup (#351, #344, tech debt)
- **close-array-gaps.AC10.1 Success:** #351 has "why" comments documenting 0-based/1-based asymmetry
- **close-array-gaps.AC10.2 Success:** #344 existing rejection tests pass, JSON multi-target works
- **close-array-gaps.AC10.3 Success:** `#[allow(dead_code)]` count reduced for array-related scaffolding
- **close-array-gaps.AC10.4 Success:** `docs/tech-debt.md` items 12 and 13 updated with new counts

---

## Codebase Verification Findings

- Confirmed: 8 `#[ignore]` tests in `array_tests.rs` (design targets 7 -- the 7 named in AC9)
- Confirmed: `range_basic` (line 1349) expects `[1.0, 2.0, 3.0, 3.0, 3.0]` (last-element-repeat, needs update to NaN fill)
- Confirmed: `range_with_expressions` (line 1361) expects `[2.0, 3.0, 4.0, 5.0, 5.0, ...]` (same, needs NaN fill update)
- Confirmed: `out_of_bounds_iteration_returns_nan` (line 2461) requires compiler changes for mismatched-size views
- Confirmed: `bounds_check_in_fast_path` (line 2588) requires compiler changes for different-sized array assignment
- Confirmed: `transpose_and_slice` (line 1383) tests `matrix'[1:3, *]` (transpose then slice)
- Confirmed: `complex_expression` (line 1427) tests `SUM(profit[*, Product.*])`. Parser already handles `DimName.*` syntax (parser/mod.rs:708-722). Blocked by compiler-level support for the full expression, not parsing.
- Confirmed: `star_to_indexed_subdimension` (line 1719) requires `parent: Option<DimensionName>` on `Dimension`. Currently missing. `compute_subdimension_relation` in `dimensions.rs:560-587` returns `None` for `Indexed/Indexed` pairs with a TODO comment.
- Confirmed: VectorElmMap 0-based at `vm.rs:2038-2044` (no "why" comment). VectorSortOrder 1-based at `vm.rs:2061` (has `1-based-index` comment but no "why"). Interpreter already has comments for both.
- Confirmed: #344 tests already pass: `json_preserves_multi_target_positional_mappings` (json.rs:3719), `mappings_takes_precedence_over_maps_to` (json.rs:3748).
- Confirmed: Tech-debt item 12 (58 dead_code suppressions, as of 2026-02-15) and item 13 (36 ignored tests, as of 2026-02-27) need count updates.
- Confirmed: `bytecode.rs:544` has stale `#[allow(dead_code)] // Array opcodes not yet emitted by compiler` -- these opcodes ARE emitted via `codegen.rs`.
- Confirmed: Multiple dead_code suppressions in `dimensions.rs` and `expr3.rs` for code that should become reachable after prior phases.

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
<!-- START_TASK_1 -->
### Task 1: Support dimension-mismatched A2A assignment with NaN fill

**Verifies:** close-array-gaps.AC9.1, close-array-gaps.AC9.3, close-array-gaps.AC9.4

**Files:**
- Modify: `src/simlin-engine/src/compiler/mod.rs` (A2A expansion logic)
- Modify: `src/simlin-engine/src/compiler/codegen.rs` (if needed for bounds-checked element reads)

**Implementation:**

When the source range in an A2A assignment produces fewer elements than the target dimension, the compiler should emit code that:
1. Iterates over the full target dimension
2. For each element index, checks if it falls within the source range bounds
3. Emits NaN for out-of-bounds reads instead of extending the last element

This affects `range_basic` (`source[1:3]` assigned to 5-element `DimA`), `out_of_bounds_iteration_returns_nan` (same pattern), and `bounds_check_in_fast_path` (3-element range assigned to 5-element target).

Update the expected values in `range_basic` and `range_with_expressions` tests from last-element-repeat to NaN fill. For example, `range_basic` expected should change from `[1.0, 2.0, 3.0, 3.0, 3.0]` to `[1.0, 2.0, 3.0, NaN, NaN]`. Use the `interpreter_result`/`vm_result_incremental` methods and `f64::is_nan()` checks for NaN positions (not `assert_interpreter_result` which uses epsilon comparison).

**Testing:**

- close-array-gaps.AC9.1: `range_basic` with updated NaN expectations
- close-array-gaps.AC9.3: `out_of_bounds_iteration_returns_nan`
- close-array-gaps.AC9.4: `bounds_check_in_fast_path`

**Verification:**

```bash
cargo test -p simlin-engine array_tests::range_basic -- --ignored
cargo test -p simlin-engine array_tests -- --ignored -k out_of_bounds
cargo test -p simlin-engine array_tests -- --ignored -k bounds_check
```

Expected: Tests pass after implementation and expected-value updates.

**Commit:** `engine: support dimension-mismatched A2A assignment with NaN fill`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Support dynamic range bounds and complex expressions in A2A context

**Verifies:** close-array-gaps.AC9.2, close-array-gaps.AC9.6

**Files:**
- Modify: `src/simlin-engine/src/compiler/context.rs` (dynamic range bounds in A2A)
- Modify: `src/simlin-engine/src/compiler/` (StarRange/qualified wildcard support)

**Implementation:**

1. **Dynamic range bounds** (AC9.2): The `range_with_expressions` test uses scalar variables as start/end for range subscripts (e.g., `arr[start_var:end_var]`). This already works inside reducer builtins (SUM context) but needs to work in the A2A assignment context. Extend the compiler to resolve dynamic range bounds during A2A iteration.

2. **Complex expression with qualified wildcard** (AC9.6): The `complex_expression` test uses `SUM(profit[*, Product.*])`. The parser already recognizes `Product.*` as `StarRange("Product")` (parser/mod.rs:708-722). The compiler needs to:
   - Lower `StarRange` to a `SparseRange` containing the elements of the `Product` subdimension
   - Ensure the SUM reduction correctly iterates over the sparse range combined with the wildcard

Check what `IndexExpr0::StarRange` lowers to in the expr0->expr1->expr2->expr3 pipeline. If the lowering is incomplete, implement the missing steps.

**Testing:**

- close-array-gaps.AC9.2: `range_with_expressions` with updated NaN expectations
- close-array-gaps.AC9.6: `complex_expression` calls `assert_compiles_incremental()`

**Verification:**

```bash
cargo test -p simlin-engine array_tests::range_with_expressions -- --ignored
cargo test -p simlin-engine array_tests::complex_expression -- --ignored
```

Expected: Both tests pass.

**Commit:** `engine: support dynamic range bounds and qualified wildcard in A2A context`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Support transpose + subscript chaining

**Verifies:** close-array-gaps.AC9.5

**Files:**
- Modify: `src/simlin-engine/src/compiler/context.rs` or `src/simlin-engine/src/compiler/subscript.rs`

**Implementation:**

The `transpose_and_slice` test uses `matrix'[1:3, *]` -- transpose the matrix, then apply a range subscript on the first dimension and wildcard on the second. The compiler needs to ensure that subscripting a transposed expression operates on the transposed view, not the original.

Check how `Expr3::Transpose` is lowered in `lower_from_expr3`. Ensure the resulting view has swapped dimensions so that subsequent `Subscript` operations index into the correct axes.

The expected output is `[1.0, 11.0, 21.0, 2.0, 12.0, 22.0]` which represents rows 1-3 of the transposed matrix (original columns).

**Testing:**

- close-array-gaps.AC9.5: `transpose_and_slice` passes

**Verification:**

```bash
cargo test -p simlin-engine array_tests::transpose_and_slice -- --ignored
```

Expected: Test passes.

**Commit:** `engine: support transpose + subscript chaining`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Add parent field to Dimension for indexed subdimensions

**Verifies:** close-array-gaps.AC9.7

**Files:**
- Modify: `src/simlin-engine/src/datamodel.rs` (Dimension struct)
- Modify: `src/simlin-engine/src/dimensions.rs:560-587` (compute_subdimension_relation for Indexed/Indexed)
- Modify: `src/simlin-engine/src/project_io.proto` (add parent field to Dimension)
- Modify: `src/simlin-engine/src/serde.rs` (serialize/deserialize parent)
- Modify: `src/simlin-engine/src/json.rs` (serialize/deserialize parent)
- Modify: `src/simlin-engine/src/xmile/dimensions.rs` (serialize/deserialize parent)

**Implementation:**

1. Add `pub parent: Option<DimensionName>` to `Dimension` in `datamodel.rs`. This field indicates which parent dimension an indexed subdimension belongs to (e.g., `SubIndex` with parent `FullIndex` means SubIndex represents elements 2-4 of FullIndex). Note: `ModelGroup` also has a `parent` field, but it represents model group hierarchy -- an unrelated concept. The `Dimension.parent` field here tracks indexed subdimension-to-parent-dimension relationships.

2. Update `compute_subdimension_relation` in `dimensions.rs` to handle `Indexed/Indexed` pairs by checking the `parent` field. The current code returns `None` with a TODO at this branch.

3. Add proto field, serde, json, and XMILE serialization for the new field (backward compat: absent = None).

4. Update MDL conversion to set `parent` when defining indexed subdimensions that reference a parent dimension.

**Testing:**

- close-array-gaps.AC9.7: `star_to_indexed_subdimension` test passes. It tests `arr[*:SubIndex] * 2` where SubIndex is a 3-element indexed subdimension of a 5-element parent.

**Verification:**

```bash
cargo test -p simlin-engine array_tests::star_to_indexed_subdimension -- --ignored
```

Expected: Test passes.

**Commit:** `engine: add parent field to Dimension for indexed subdimensions`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_5 -->
### Task 5: Remove #[ignore] from enabled tests

**Verifies:** close-array-gaps.AC9.1 through AC9.7

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs` (7 test ignore annotations)

**Implementation:**

Remove `#[ignore]` from the 7 tests that should now pass:
1. `range_basic` (line 1349)
2. `range_with_expressions` (line 1361)
3. `out_of_bounds_iteration_returns_nan` (line 2461)
4. `bounds_check_in_fast_path` (line 2588)
5. `transpose_and_slice` (line 1383)
6. `complex_expression` (line 1427)
7. `star_to_indexed_subdimension` (line 1719)

Remove associated TODO comments that explained why tests were ignored.

**Testing:**

All 7 tests run as part of the normal test suite (not requiring `--ignored` flag).

**Verification:**

```bash
cargo test -p simlin-engine array_tests
```

Expected: All 7 previously-ignored tests pass.

**Commit:** `engine: enable 7 previously-ignored array_tests`
<!-- END_TASK_5 -->

<!-- START_SUBCOMPONENT_B (tasks 6-7) -->
<!-- START_TASK_6 -->
### Task 6: Add "why" comments for #351 and verify #344

**Verifies:** close-array-gaps.AC10.1, close-array-gaps.AC10.2

**Files:**
- Modify: `src/simlin-engine/src/vm.rs:2038-2044` (VectorElmMap 0-based comment)
- Modify: `src/simlin-engine/src/vm.rs:2061` (VectorSortOrder 1-based comment)

**Implementation:**

1. **#351 VectorElmMap (vm.rs:2038-2044):** Add a "why" comment explaining the 0-based convention:
```rust
// VectorElmMap uses 0-based offset indexing: offset 0 means "element at
// position 0 of the source array." This matches Vensim's VECTOR ELM MAP
// semantics where the offset array contains zero-based indices.
```

2. **#351 VectorSortOrder (vm.rs:2061):** Enhance the existing comment to explain the asymmetry:
```rust
// VectorSortOrder returns 1-based rank indices: rank 1 means "this element
// is first in sort order." This matches Vensim's VECTOR SORT ORDER semantics.
// Note: this is intentionally 1-based, unlike VectorElmMap which uses 0-based
// offsets. The asymmetry reflects Vensim's original design.
```

3. **#344 verification:** Confirm the existing passing tests cover the issue:
   - `json_preserves_multi_target_positional_mappings` (json.js:3719) -- passing
   - `mappings_takes_precedence_over_maps_to` (json.js:3748) -- passing

   No code changes needed. The issue can be closed with a reference to these tests.

**Testing:**

- close-array-gaps.AC10.1: Comments are present and accurate
- close-array-gaps.AC10.2: Existing tests pass

**Verification:**

```bash
cargo test -p simlin-engine
```

Expected: All tests pass (comment-only changes + verification).

**Commit:** `engine: add why-comments for VectorElmMap/VectorSortOrder indexing asymmetry`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Clean up dead code suppressions and update tech-debt.md

**Verifies:** close-array-gaps.AC10.3, close-array-gaps.AC10.4

**Files:**
- Modify: `src/simlin-engine/src/bytecode.rs:544` (remove stale dead_code comment)
- Modify: various files in `src/simlin-engine/src/` (remove dead_code suppressions for now-reachable code)
- Modify: `docs/tech-debt.md` (update items 12 and 13)

**Implementation:**

1. **bytecode.rs:544**: Remove the `#[allow(dead_code)] // Array opcodes not yet emitted by compiler` annotation. All array opcodes are now emitted by `codegen.rs`.

2. **Systematic cleanup:** Run `rg '#\[allow\(dead_code\)\]' --type rust src/simlin-engine/src/ -c` to get current counts. For each suppression in array-related files, check if the code is now reachable after Phases 1-5. Remove suppressions where code is reachable. Key candidates:
   - `bytecode.rs:1080,1097` -- ByteCodeContext fields/methods used by array bytecode
   - `dimensions.rs` -- SubdimensionRelation, RelationshipCache, etc. if now used
   - `expr3.rs` -- StaticSubscript, TempArrayElement if now reached in pass 2

3. **Update tech-debt.md item 12:** Run the measure command and update the count:
```bash
rg '#\[allow\(dead_code\)\]' --type rust src/simlin-engine/src/ -c
```
Update the count and date.

4. **Update tech-debt.md item 13:** Run the measure command and update the count:
```bash
rg '#\[ignore\]' --type rust src/simlin-engine/ -c
```
Update the count and date. The ignored test count should be reduced by 7 (from 36 to ~29, plus any other tests un-ignored in earlier phases).

**Testing:**

- close-array-gaps.AC10.3: dead_code count reduced
- close-array-gaps.AC10.4: tech-debt.md updated with accurate counts

**Verification:**

```bash
cargo build -p simlin-engine
cargo test -p simlin-engine
```

Expected: Compiles without warnings about unused code (for cleaned-up suppressions). All tests pass.

**Commit:** `engine: clean up dead code suppressions and update tech-debt counts`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_B -->
