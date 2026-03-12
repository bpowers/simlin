# Close Array Gaps Implementation Plan -- Phase 1

**Goal:** Split the conflated `preserve_wildcards_for_iteration` flag and add empty-view guards to all array reducers, removing latent bugs before subsequent compiler work.

**Architecture:** Two independent fixes in the compiler and execution engines. The flag split adds a new `promote_active_dim_ref` boolean to `Context` so reducer builtins (SUM, MEAN, etc.) no longer promote `ActiveDimRef` subscripts to `Wildcard`. The empty-view guards add explicit zero-size checks to all six array reducer operations in both the VM and interpreter.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 6 phases from original design (this is phase 1 of 6)

**Codebase verified:** 2026-03-11

**Testing references:** See `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` (lines 94-112) for test index; `/home/bpowers/src/simlin/docs/dev/rust.md` for Rust testing standards; `/home/bpowers/src/simlin/docs/dev/commands.md` for test commands.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### close-array-gaps.AC1: preserve_wildcards flag split (#365)
- **close-array-gaps.AC1.1 Success:** Reducer builtins (SUM, MEAN, MIN, MAX, SIZE, STDDEV) preserve Wildcard/SparseRange/Range ops but do NOT promote ActiveDimRef to Wildcard
- **close-array-gaps.AC1.2 Success:** Vector builtins (VectorElmMap, VectorSortOrder, VectorSelect, AllocateAvailable) promote ActiveDimRef to Wildcard but do NOT preserve reducer-style ops
- **close-array-gaps.AC1.3 Regression:** Nested `SUM(VECTOR SORT ORDER(...))` produces correct results (not corrupted by conflated flag)

### close-array-gaps.AC2: Empty-view array reducer guards (#388)
- **close-array-gaps.AC2.1 Success:** MEAN, MIN, MAX, STDDEV return NaN for zero-size views in both VM and interpreter
- **close-array-gaps.AC2.2 Success:** SUM returns 0.0 for zero-size views in both VM and interpreter
- **close-array-gaps.AC2.3 Success:** SIZE returns 0 for zero-size views in both VM and interpreter

---

## Codebase Verification Findings

- Confirmed: `preserve_wildcards_for_iteration: bool` field at `src/simlin-engine/src/compiler/context.rs:45`
- Confirmed: `with_preserved_wildcards()` constructor at line 988
- Confirmed: `has_iteration_preserving_ops` check at lines 1285-1293 includes `ActiveDimRef` alongside `Wildcard | SparseRange | Range`
- Discrepancy: 10 call sites in `lower_builtin_expr3` (not ~12): 6 reducer (Max:1880, Mean:1891, Min:1901, Size:1970, Stddev:1974, Sum:1978) + 4 vector (VectorSelect:1982, VectorElmMap:1992, VectorSortOrder:1999, AllocateAvailable:2006)
- Confirmed: `build_view_from_ops` in `subscript.rs:306` correctly resolves `ActiveDimRef` to a concrete offset when not promoted -- this means the preserve path works correctly when ActiveDimRef is left as-is
- VM empty-view: ArrayMax returns `NEG_INFINITY`, ArrayMin returns `INFINITY` (need NaN); ArrayMean/ArrayStddev already produce NaN incidentally via IEEE div-by-zero (add explicit guards for clarity); ArraySum returns 0.0 (correct); ArraySize returns 0 (correct)
- Interpreter empty-view: `array_mean` returns 0.0 at line 582 (needs NaN); `array_stddev` returns 0.0 at line 593 (needs NaN for size==0); Min returns `INFINITY` (needs NaN); Max returns `NEG_INFINITY` (needs NaN); Sum returns 0.0 (correct); Size returns 0 (correct)
- Regression tests exist: `nested_vector_sort_order_inside_sum_in_array_context_{interpreter,vm}` in `tests/compiler_vector.rs:198,209`
- No existing empty-view tests found

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Add promote_active_dim_ref field and split Subscript lowering

**Verifies:** close-array-gaps.AC1.1, close-array-gaps.AC1.2

**Files:**
- Modify: `src/simlin-engine/src/compiler/context.rs:36-46` (Context struct)
- Modify: `src/simlin-engine/src/compiler/context.rs:70-84` (Context::new)
- Modify: `src/simlin-engine/src/compiler/context.rs:86-98` (with_active_context)
- Modify: `src/simlin-engine/src/compiler/context.rs:986-997` (with_preserved_wildcards)
- Modify: `src/simlin-engine/src/compiler/context.rs:1280-1316` (has_iteration_preserving_ops + preserve path)

**Implementation:**

1. Add `promote_active_dim_ref: bool` field to the `Context` struct (after `preserve_wildcards_for_iteration` at line 45). Initialize to `false` in `Context::new` (line 82). Propagate through `with_active_context` (line 97).

2. Add a new constructor `with_vector_builtin_wildcards(&self) -> Self` alongside `with_preserved_wildcards()`. Both set `preserve_wildcards_for_iteration: true`. The new one additionally sets `promote_active_dim_ref: true`. Keep `with_preserved_wildcards()` for reducers -- it already sets `preserve_wildcards_for_iteration: true` and the new field defaults to `false`.

3. Split the `has_iteration_preserving_ops` check at lines 1285-1293 into two separate checks:

```rust
let has_wildcard_ops = operations.iter().any(|op| {
    matches!(op, IndexOp::Wildcard | IndexOp::SparseRange(_) | IndexOp::Range(_, _))
});
let has_active_dim_ref = operations.iter().any(|op| {
    matches!(op, IndexOp::ActiveDimRef(_))
});

let preserve_for_iteration = self.preserve_wildcards_for_iteration
    && (has_wildcard_ops || (self.promote_active_dim_ref && has_active_dim_ref));
```

4. In the preserve path (lines 1303-1308), only promote ActiveDimRef when `promote_active_dim_ref` is true:

```rust
let preserved_ops: Vec<IndexOp> = operations
    .iter()
    .map(|op| match op {
        IndexOp::ActiveDimRef(_) if self.promote_active_dim_ref => IndexOp::Wildcard,
        other => other.clone(),
    })
    .collect();
```

When `promote_active_dim_ref` is false (reducer context), `ActiveDimRef` passes through to `build_view_from_ops` which resolves it to a concrete element offset via `subscript.rs:363-402`.

**Testing:**

Tests are covered in Task 3 (regression verification) -- the flag split changes behavior only when ActiveDimRef is present inside reducer builtins, and the existing regression tests in compiler_vector.rs verify nested SUM(VECTOR SORT ORDER(...)) still works.

**Verification:**

```bash
cargo test -p simlin-engine
```

Expected: All existing tests pass (this is a refactor that changes internal behavior only for the case where ActiveDimRef appears inside a reducer, which is currently tested indirectly by the nested tests).

**Commit:** `engine: split preserve_wildcards flag for reducer vs vector builtins`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update vector builtin call sites to use new constructor

**Verifies:** close-array-gaps.AC1.2, close-array-gaps.AC1.3

**Files:**
- Modify: `src/simlin-engine/src/compiler/context.rs:1981-2020` (4 vector builtin call sites in lower_builtin_expr3)

**Implementation:**

Change the 4 vector builtin call sites from `with_preserved_wildcards()` to `with_vector_builtin_wildcards()`:

- Line 1982: `VectorSelect` -- change `self.with_preserved_wildcards()` to `self.with_vector_builtin_wildcards()`
- Line 1992: `VectorElmMap` -- same change
- Line 1999: `VectorSortOrder` -- same change
- Line 2006: `AllocateAvailable` -- same change

The 6 reducer call sites (Max:1880, Mean:1891, Min:1901, Size:1970, Stddev:1974, Sum:1978) keep `with_preserved_wildcards()`.

**Testing:**

- close-array-gaps.AC1.3: Existing regression tests `nested_vector_sort_order_inside_sum_in_array_context_interpreter` and `nested_vector_sort_order_inside_sum_in_array_context_vm` at `tests/compiler_vector.rs:198,209` verify the nested case still works.
- Run the full test suite to verify no regressions.

**Verification:**

```bash
cargo test -p simlin-engine --features testing --test compiler_vector
cargo test -p simlin-engine
```

Expected: All tests pass, including the nested SUM(VECTOR SORT ORDER(...)) regression tests.

**Commit:** `engine: use vector builtin context for VectorElmMap/VectorSortOrder/VectorSelect/AllocateAvailable`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add flag split unit tests

**Verifies:** close-array-gaps.AC1.1, close-array-gaps.AC1.2

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs` (add new test module)

**Implementation:**

Add tests in `array_tests.rs` that exercise the split flag behavior using the `TestProject` builder:

1. **Reducer does not promote ActiveDimRef:** Build a model with an arrayed variable `vals[DimA]` (3 elements: 10, 20, 30) and a scalar `result = SUM(vals[DimA])`. The SUM should sum all elements (Wildcard case). Then build a model where an arrayed output `partial_sum[DimB]` uses `SUM(matrix[DimA, DimB])` -- this should sum over DimA while DimB iterates, NOT sum the entire matrix. Verify via both `assert_interpreter_result` and `assert_vm_result_incremental`.

2. **Vector builtin promotes ActiveDimRef:** Build a model with `vals[DimA]` and `result[DimA] = VECTOR SORT ORDER(vals[DimA], 1)`. The `vals[DimA]` inside the vector builtin should be promoted to the full array view. Verify via both execution paths. (This is already covered by existing tests in compiler_vector.rs, but having a focused unit test documents the intent.)

**Testing:**

- close-array-gaps.AC1.1: Test verifies reducer behavior with ActiveDimRef
- close-array-gaps.AC1.2: Test verifies vector builtin behavior with ActiveDimRef

**Verification:**

```bash
cargo test -p simlin-engine array_tests
```

Expected: New tests pass.

**Commit:** `engine: add unit tests for flag split behavior`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->
<!-- START_TASK_4 -->
### Task 4: Add VM empty-view guards

**Verifies:** close-array-gaps.AC2.1, close-array-gaps.AC2.2, close-array-gaps.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/vm.rs:1736-1801` (6 array reducer opcode handlers)

**Implementation:**

Add explicit zero-size view checks at the start of each array reducer opcode handler. The `reduce_view` helper already handles `!is_valid` by returning NaN, but does NOT check for zero-size valid views.

For `Opcode::ArrayMax` (line 1743), `Opcode::ArrayMin` (line 1756): Add a zero-size guard that pushes NaN and continues. These currently return `NEG_INFINITY` and `INFINITY` respectively for empty views.

```rust
Opcode::ArrayMax {} => {
    let view = view_stack.last().unwrap();
    if view.size() == 0 {
        stack.push(f64::NAN);
    } else {
        let max = Self::reduce_view(
            temp_storage, view, curr, context,
            |acc, v| if v > acc { v } else { acc },
            f64::NEG_INFINITY,
        );
        stack.push(max);
    }
}
```

Same pattern for ArrayMin.

For `Opcode::ArrayMean` (line 1769), `Opcode::ArrayStddev` (line 1777): These already produce NaN incidentally through IEEE division by zero, but add explicit guards for clarity and to document the intent:

```rust
Opcode::ArrayMean {} => {
    let view = view_stack.last().unwrap();
    if view.size() == 0 {
        stack.push(f64::NAN);
    } else {
        let sum = Self::reduce_view(temp_storage, view, curr, context, |acc, v| acc + v, 0.0);
        let count = view.size() as f64;
        stack.push(sum / count);
    }
}
```

Same pattern for ArrayStddev.

For `Opcode::ArraySum` (line 1736): Already returns 0.0 for empty views (the init value). No change needed, but add a comment documenting this is intentional.

For `Opcode::ArraySize` (line 1798): Already returns 0 for empty views. No change needed.

**Testing:**

Tests in Task 6.

**Verification:**

```bash
cargo test -p simlin-engine
```

Expected: All existing tests pass.

**Commit:** `engine: add explicit empty-view guards to VM array reducers`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Fix interpreter empty-view behavior

**Verifies:** close-array-gaps.AC2.1, close-array-gaps.AC2.2, close-array-gaps.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/interpreter.rs:578-608` (array_mean, array_stddev)
- Modify: `src/simlin-engine/src/interpreter.rs:1013-1056` (Min, Max single-arg handlers)

**Implementation:**

1. `array_mean` (line 579): Change `if size == 0 { return 0.0; }` to `if size == 0 { return f64::NAN; }`.

2. `array_stddev` (line 590): Separate the size==0 and size==1 cases. Currently `if size <= 1 { return 0.0; }`. Change to:
```rust
if size == 0 {
    return f64::NAN;
}
if size <= 1 {
    return 0.0;
}
```

3. `BuiltinFn::Min` single-arg (line 1013-1023): Add a zero-size check before calling `reduce_array`. Use `get_array_size` to check, and return NaN for empty arrays:
```rust
if b.is_none() {
    let size = self.get_array_size(a);
    if size == 0 {
        f64::NAN
    } else {
        self.reduce_array(a, f64::INFINITY, |acc, val| if val < acc { val } else { acc })
    }
}
```

4. `BuiltinFn::Max` single-arg (line 1043-1049): Same pattern as Min, returning NaN for empty arrays.

Sum (line 1247) already returns 0.0 (correct). Size (line 1250) already returns 0 (correct). No changes needed for those.

**Testing:**

Tests in Task 6.

**Verification:**

```bash
cargo test -p simlin-engine
```

Expected: All existing tests pass.

**Commit:** `engine: fix interpreter empty-view behavior for array reducers`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Add empty-view unit tests

**Verifies:** close-array-gaps.AC2.1, close-array-gaps.AC2.2, close-array-gaps.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs` (add new test module)

**Implementation:**

Add a test module in `array_tests.rs` that verifies empty-view behavior for all 6 reducers. The challenge is creating a zero-size array view in a test. There are two approaches:

**Approach A (preferred):** Use a model with a subdimension/subrange that resolves to zero elements. If the engine supports a dimension with zero elements, create `SUM(x[EmptySubrange])` and verify the result.

**Approach B:** If zero-element subranges aren't supported at the model level, test at the VM/interpreter level directly by constructing views with zero-size dimensions. This may require calling internal functions directly.

For each of the 6 reducers (SUM, MEAN, MIN, MAX, SIZE, STDDEV), verify:
- close-array-gaps.AC2.1: MEAN, MIN, MAX, STDDEV return NaN -- **must** use `interpreter_result()` / `vm_result_incremental()` raw value methods and check with `f64::is_nan()`. Do NOT use `assert_interpreter_result` or `assert_vm_result_incremental` for NaN positions because those helpers use epsilon comparison (`(a - b).abs() < epsilon`) which always fails for NaN since `NaN != NaN`.
- close-array-gaps.AC2.2: SUM returns 0.0 -- can use `assert_interpreter_result` / `assert_vm_result_incremental`
- close-array-gaps.AC2.3: SIZE returns 0 -- can use `assert_interpreter_result` / `assert_vm_result_incremental`

Both interpreter and VM paths must be tested.

**Testing:**

- close-array-gaps.AC2.1: Tests verify NaN return for MEAN/MIN/MAX/STDDEV on empty views
- close-array-gaps.AC2.2: Test verifies SUM returns 0.0 on empty views
- close-array-gaps.AC2.3: Test verifies SIZE returns 0 on empty views

**Verification:**

```bash
cargo test -p simlin-engine array_tests
```

Expected: All new empty-view tests pass.

**Commit:** `engine: add empty-view unit tests for all array reducers`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->
