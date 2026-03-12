# Close Array Gaps Implementation Plan -- Phase 3

**Goal:** Allow VECTOR ELM MAP and other array-producing builtins to accept source arrays from dimensions unrelated to the output, matching Vensim semantics where `b[B1]` inside VectorElmMap means "full b array."

**Architecture:** Two related compiler fixes: (1) in the `Expr3::Subscript` lowering path, detect when `promote_active_dim_ref` is true (from Phase 1's flag split) and prevent Single-collapsed scalar views from losing their array identity; (2) fix the VM incremental compilation path for VECTOR SORT ORDER cross-dimension scenarios.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 6 phases from original design (this is phase 3 of 6)

**Codebase verified:** 2026-03-11

**Testing references:** See `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` for test index.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### close-array-gaps.AC5: VECTOR ELM MAP cross-dimension (#357, #358)
- **close-array-gaps.AC5.1 Success:** `b[B1]` as source in DimA context treats as full b array (Vensim semantics)
- **close-array-gaps.AC5.2 Success:** `d[DimA,B1]` partial collapse with DimB broadcast works correctly
- **close-array-gaps.AC5.3 Success:** `x[three]` scalar source with cross-dimension offset works
- **close-array-gaps.AC5.4 Success:** `simulates_vector_xmile` integration test passes
- **close-array-gaps.AC5.5 Success:** `simulates_vector_simple_mdl` runs full (VM + interpreter), not interpreter-only

---

## Codebase Verification Findings

- Confirmed: `promote_active_dim_ref` does NOT exist yet in the codebase -- Phase 1 adds it. Phase 3 depends on Phase 1.
- Confirmed: `b[B1]` normalizes to `IndexOp::Single(0)` at `compiler/subscript.rs:257-267`, collapsing DimB to scalar
- Confirmed: Scalar collapse returns early at `compiler/context.rs:1318-1319` (`view.dims.is_empty()` → `Expr::Var`), BEFORE reaching MismatchedDimensions guard at line 1397
- Confirmed: MismatchedDimensions guard at `context.rs:1397-1399` fires for cases like `d[DimA,B1]` where partial dimension collapse leaves a mismatched view dimension count
- Confirmed: `simulates_vector_xmile` is commented out at `tests/simulate.rs:888` inside `TEST_SDEVERYWHERE_MODELS` array
- Confirmed: `simulates_vector_simple_mdl` at `tests/simulate.rs:1000-1008` uses `simulate_mdl_path_interpreter_only` due to VM incremental path bug
- Confirmed: VM bug: `m[DimA] = VECTOR SORT ORDER(h[DimA], 0)` produces wrong result `m[A3]=0` instead of `m[A3]=2` in VM incremental path
- Confirmed: vector_simple.mdl tests: `c[DimA] = 10 + VECTOR ELM MAP(b[B1], a[DimA])` (cross-dim), `f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])` (partial collapse), `m[DimA] = VECTOR SORT ORDER(h[DimA], 0)` (sort order)
- Confirmed: vector.xmile additionally tests `y[DimA] = VECTOR ELM MAP(x[three], (DimA - 1))` (scalar source with cross-dimension offset)

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Handle cross-dimension source arrays in vector builtin context

**Verifies:** close-array-gaps.AC5.1, close-array-gaps.AC5.2, close-array-gaps.AC5.3

**Files:**
- Modify: `src/simlin-engine/src/compiler/context.rs:1300-1400` (Subscript lowering in lower_from_expr3)

**Implementation:**

Two changes in the `Expr3::Subscript` arm of `lower_from_expr3`, within the `active_subscript`/`active_dimension` context block:

**Change 1: Promote collapsed scalar to full array (line 1318)**

When `promote_active_dim_ref` is true and `view.dims.is_empty()` (all dimensions collapsed to Single), the source variable should be treated as its full array. Instead of returning `Expr::Var(off + view.offset, *loc)`, rebuild the operations replacing all `IndexOp::Single` entries with `IndexOp::Wildcard` and return the full array view via `build_view_from_ops`.

This handles the `b[B1]` case: B1 collapses DimB to Single, making view empty. With `promote_active_dim_ref` true (inside VectorElmMap), Single(0) becomes Wildcard, restoring the full `b` array view.

Only apply this when the collapsed Single came from a named-element subscript (not from ActiveDimRef resolution). The operations vector still contains the original `IndexOp::Single` values, so check if any exist before promoting.

**Change 2: Suppress MismatchedDimensions for partial collapse (line 1397-1399)**

When `promote_active_dim_ref` is true, skip the guard `if !all_name_matching && view.dims.len() != active_dims.len()`. The dimension mismatch is expected -- the source array lives in a different dimension space than the output.

This handles the `d[DimA,B1]` case: after B1 collapses one axis, the view has DimA only, but active context has DimA×DimB. The mismatch is correct -- the VectorElmMap will handle dimension broadcasting.

Also consider the `x[three]` case from the full vector model: `x` is 5-element DimX, subscripted to `x[three]` (Single collapse to scalar). With promote_active_dim_ref, this gets promoted to full `x[DimX]` array. The VectorElmMap offset expression `(DimA - 1)` then indexes into the full 5-element array.

**Testing:**

Tests in Task 3 via integration tests.

**Verification:**

```bash
cargo build -p simlin-engine
```

Expected: Compiles without errors.

**Commit:** `engine: handle cross-dimension source arrays in vector builtin context`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Fix VM incremental path for VECTOR SORT ORDER cross-dimension

**Verifies:** close-array-gaps.AC5.5

**Files:**
- Modify: `src/simlin-engine/src/compiler/mod.rs` (A2A hoisting logic, around line 1509+)

**Implementation:**

The bug: `m[DimA] = VECTOR SORT ORDER(h[DimA], 0)` produces wrong result `m[A3]=0` instead of `m[A3]=2` in the VM incremental path.

**This is an investigation-first task.** The root cause is not yet known. Before writing any code, diagnose the bug by following this investigation checklist:

1. [ ] Read `expand_a2a_with_hoisting` in `compiler/mod.rs` (line ~1509) to understand how array-producing builtins are hoisted to `AssignTemp`
2. [ ] Read `expression_depends_on_active_dimension` to understand how it decides what to hoist
3. [ ] Trace the lowering path for `VECTOR SORT ORDER(h[DimA], 0)` through the compiler -- add debug logging if needed
4. [ ] Compare the bytecode emitted for `m[A1]`, `m[A2]`, `m[A3]` to identify where `m[A3]=0` comes from
5. [ ] Check `lower_preserving_dimensions` (referenced in context.rs line 1522-1530) -- is the array argument being lowered correctly?
6. [ ] Check `AssignTemp` / `TempArrayElement` emission -- is the element indexing correct?
7. [ ] Check the direction argument `0` (DESCENDING) -- is it being treated as a literal or resolved incorrectly?

The interpreter path works correctly because it uses direct AST-walking evaluation rather than the hoisted bytecode. Compare interpreter and VM execution paths to isolate the divergence.

Fix the bug so `simulate_mdl_path` (both VM + interpreter) produces correct results matching the reference `.dat` output.

**Testing:**

Tests in Task 3.

**Verification:**

```bash
cargo test -p simlin-engine --features file_io,testing --test simulate simulates_vector_simple_mdl
```

Expected: Fails before fix, passes after (will upgrade to full path in Task 3).

**Commit:** `engine: fix VM incremental path for VECTOR SORT ORDER cross-dimension`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_3 -->
### Task 3: Un-comment and upgrade vector integration tests

**Verifies:** close-array-gaps.AC5.4, close-array-gaps.AC5.5

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:882-888` (uncomment vector.xmile path)
- Modify: `src/simlin-engine/tests/simulate.rs:1000-1008` (upgrade vector_simple to full)

**Implementation:**

1. In `TEST_SDEVERYWHERE_MODELS` array at line 888, uncomment the vector.xmile path:
```rust
"test/sdeverywhere/models/vector/vector.xmile",
```
Remove the explanatory comment block above it (lines 882-887).

2. In `simulates_vector_simple_mdl` at line 1005, change `simulate_mdl_path_interpreter_only` to `simulate_mdl_path`. Remove or update the comment explaining the interpreter-only limitation (lines 1001-1004).

**Testing:**

- close-array-gaps.AC5.4: `simulates_vector_xmile` runs as part of `simulates_arrayed_models_correctly` and passes (both interpreter and VM)
- close-array-gaps.AC5.5: `simulates_vector_simple_mdl` runs both VM + interpreter against reference output

**Verification:**

```bash
cargo test -p simlin-engine --features file_io,testing --test simulate simulates_vector_simple_mdl
cargo test -p simlin-engine --features file_io,testing --test simulate simulates_arrayed_models_correctly
```

Expected: Both tests pass with VM + interpreter paths validated.

**Commit:** `engine: enable vector cross-dimension integration tests`
<!-- END_TASK_3 -->
