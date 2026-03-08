# Assert-Compiles Migration -- Phase 1: @N Position Syntax in Scalar Context

**Goal:** Make `arr[@N]` work on the incremental compilation path when the LHS is a scalar variable, and when mixed with wildcards.

**Architecture:** Modify `lower_index_expr3` in `context.rs` to resolve `DimPosition(pos)` to a concrete 1-based offset when in scalar context (no active A2A dimension), rather than returning `ArrayReferenceNeedsExplicitSubscripts`. For mixed contexts (DimPosition + wildcard, like `cube[@1, *, @3]`), the same resolution applies when the active subscript at position `pos` doesn't match the target dimension.

**Tech Stack:** Rust (simlin-engine)

**Scope:** Phase 1 of 3 from design plan

**Codebase verified:** 2026-03-08

**Reference files for executor:**
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` -- engine architecture and module map
- `/home/bpowers/src/simlin/CLAUDE.md` -- project-wide development standards

---

## Acceptance Criteria Coverage

This phase implements and tests:

### assert-compiles-migration.AC1: @N position syntax works on incremental path
- **assert-compiles-migration.AC1.1 Success:** `arr[@1]` in scalar context compiles and selects the first dimension element
- **assert-compiles-migration.AC1.2 Success:** `cube[@1, *, @3]` with mixed position/wildcard compiles on incremental path
- **assert-compiles-migration.AC1.3 Failure:** `arr[@0]` produces a compile error (1-based, 0 is invalid)
- **assert-compiles-migration.AC1.4 Failure:** `arr[@N]` where N exceeds dimension size produces a compile error

---

## Codebase Verification Findings

- **Confirmed:** `lower_index_expr3` at `src/simlin-engine/src/compiler/context.rs:2062-2070` handles `IndexExpr3::DimPosition(pos, dim_loc)` starting at line 2131. When `self.active_dimension.is_none()`, it returns `sim_err!(ArrayReferenceNeedsExplicitSubscripts, ...)` at line 2134. This is the fix site.
- **Discrepancy:** Design says "resolve to `IndexExpr3::Named(elements[pos-1])`" but `IndexExpr3::Named` does not exist. The correct approach: resolve to `SubscriptIndex::Single(Expr::Const((pos) as f64, loc))` (1-based offset), consistent with how the A2A path resolves at line 2148.
- **Confirmed:** `dimension_position_single` test at `array_tests.rs:334` (inside `mod dimension_position_tests` starting at line 330). `dimension_position_and_wildcard` test at `array_tests.rs:1357`.
- **Confirmed:** `Dimension::Named(_, named_dim)` has `named_dim.elements: Vec<CanonicalElementName>` and `Dimension::Indexed(_, size)` has `size: u32`. `Dimension::len()` exists at `dimensions.rs:108` and returns the element count for both variants.
- **Confirmed:** For out-of-range errors, existing code uses `ErrorCode::MismatchedDimensions` (consistent with A2A path at line 2142). No dedicated `ArrayIndexOutOfBounds` variant.
- **Gap:** No `assert_compile_error_incremental()` method exists on `TestProject`. For error tests (AC1.3, AC1.4), call `compile_incremental()` and verify the error message contains the expected context (e.g., the variable name) to avoid false positives from unrelated compilation errors.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Modify `lower_index_expr3` to resolve scalar @N

**Verifies:** assert-compiles-migration.AC1.1, assert-compiles-migration.AC1.2

**Files:**
- Modify: `src/simlin-engine/src/compiler/context.rs:2131-2161` (the `DimPosition` match arm in `lower_index_expr3`)

**Implementation:**

Replace the `IndexExpr3::DimPosition(pos, dim_loc)` match arm (lines 2131-2161) with logic that handles three cases:

1. **Scalar context** (`self.active_dimension.is_none()`): Resolve `@N` to a concrete 1-based element offset in `dims[i]`. Check bounds: `pos == 0` or `pos > dim_size` produces `MismatchedDimensions`. Otherwise return `SubscriptIndex::Single(Expr::Const(*pos as f64, *dim_loc))`.

2. **A2A context, within active subscript range, subscript matches dimension** (existing behavior): `active_subscripts[pos-1]` resolved via `dim.get_offset(subscript)` returns the concrete offset. This is the existing dimension-reordering path (e.g., `matrix[@2, @1]`).

3. **A2A context, but subscript doesn't match dimension or pos out of range** (new fallback for mixed cases like `cube[@1, *, @3]`): Fall back to the same concrete element offset as the scalar case.

Bounds check (applies to scalar context and A2A fallback):
```rust
let pos_val = *pos as usize;
if pos_val == 0 || pos_val > dims[i].len() {
    return sim_err!(MismatchedDimensions, id.as_str().to_string());
}
```

**Verification:**
Run: `cargo test -p simlin-engine dimension_position`
Expected: Existing `dimension_position_reorder` and `dimension_position_3d` tests still pass (they use the A2A path unchanged).

**Commit:** `engine: resolve @N position syntax to concrete offset in scalar context`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Switch existing tests to `assert_compiles_incremental()`

**Verifies:** assert-compiles-migration.AC1.1, assert-compiles-migration.AC1.2

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs:334` (`dimension_position_single` test)
- Modify: `src/simlin-engine/src/array_tests.rs:1357` (`dimension_position_and_wildcard` test)

**Implementation:**

In `dimension_position_single`: change `project.assert_compiles()` to `project.assert_compiles_incremental()`. Remove the comment about @N not being supported on the incremental path. Keep `assert_sim_builds()` and `assert_scalar_result()` calls unchanged (they use the AST interpreter for cross-validation).

In `dimension_position_and_wildcard`: change `.assert_compiles()` to `.assert_compiles_incremental()`. Remove the comment about @N not being supported.

**Verification:**
Run: `cargo test -p simlin-engine dimension_position_single`
Run: `cargo test -p simlin-engine dimension_position_and_wildcard`
Expected: Both pass.

**Commit:** `engine: switch @N position tests to incremental path`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add error tests for out-of-range @N

**Verifies:** assert-compiles-migration.AC1.3, assert-compiles-migration.AC1.4

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs` (add new tests in `dimension_position_tests` module)

**Implementation:**

Add two new tests in the `dimension_position_tests` module that verify the incremental compiler rejects invalid @N positions:

- `dimension_position_zero_is_error`: Build a `TestProject` with an indexed dimension (size 3), an array, and a scalar aux using `arr[@0]`. Assert `project.compile_incremental()` returns `Err` and the error message contains the variable name (`first_elem` or similar) to confirm the error is specifically about the @0 reference, not an unrelated compilation issue.

- `dimension_position_out_of_range_is_error`: Build a `TestProject` with an indexed dimension (size 3), an array, and a scalar aux using `arr[@5]`. Assert `project.compile_incremental()` returns `Err` and the error message contains the variable name to confirm it is specifically about the out-of-range @N.

Follow the existing `TestProject` builder pattern from tests like `dimension_position_single`.

**Verification:**
Run: `cargo test -p simlin-engine dimension_position`
Expected: All dimension_position tests pass (existing + new).

**Commit:** `engine: add error tests for out-of-range @N position syntax`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->
