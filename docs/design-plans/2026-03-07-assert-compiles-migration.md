# Assert-Compiles Migration Design

## Summary

Simlin's simulation engine has two compilation paths: a legacy monolithic path (`compile()`) and the current incremental path (`compile_incremental()`) built on salsa for fine-grained caching. Twenty-six tests still exercise the monolithic path through the `assert_compiles()` helper, keeping the old code alive. This plan eliminates that technical debt by migrating every test to `assert_compiles_incremental()` and then deleting the dead monolithic helpers entirely.

Twenty-three of the 26 tests already pass on the incremental path and need only a mechanical one-line change. The remaining three expose two feature gaps in the incremental compiler: (1) the `@N` position syntax (e.g., `arr[@1]`) fails when the left-hand side is scalar because the subscript lowering pipeline does not resolve positional indices to concrete element names outside of an apply-to-all context, and (2) the `MEAN` builtin rejects dynamic-range subscripts that other array-reduce builtins like `SUM` and `STDDEV` already handle, due to an overly narrow pattern match in the codegen layer. The plan fixes both gaps with targeted changes -- resolving `@N` to named elements early in the AST lowering chain, and extracting a shared `emit_array_reduce` helper that all six array-reduce builtins use -- then switches the remaining tests and removes the old code.

## Definition of Done

All 26 tests currently using `assert_compiles()` are switched to `assert_compiles_incremental()` and pass. This requires: (1) switching the 23 already-working tests, (2) implementing incremental support for `@N` position syntax and `MEAN` with dynamic ranges so the remaining 3 tests pass, and (3) deleting `assert_compiles()` and any code that becomes dead after its removal (but keeping the AST interpreter and its supporting infrastructure).

## Acceptance Criteria

### assert-compiles-migration.AC1: @N position syntax works on incremental path
- **assert-compiles-migration.AC1.1 Success:** `arr[@1]` in scalar context compiles and selects the first dimension element
- **assert-compiles-migration.AC1.2 Success:** `cube[@1, *, @3]` with mixed position/wildcard compiles on incremental path
- **assert-compiles-migration.AC1.3 Failure:** `arr[@0]` produces a compile error (1-based, 0 is invalid)
- **assert-compiles-migration.AC1.4 Failure:** `arr[@N]` where N exceeds dimension size produces a compile error

### assert-compiles-migration.AC2: MEAN with dynamic ranges works on incremental path
- **assert-compiles-migration.AC2.1 Success:** `MEAN(data[start_idx:end_idx])` with variable bounds compiles and simulates correctly
- **assert-compiles-migration.AC2.2 Success:** Existing SUM, SIZE, STDDEV, VMIN, VMAX behavior unchanged after refactoring to shared helper

### assert-compiles-migration.AC3: assert_compiles fully removed
- **assert-compiles-migration.AC3.1 Success:** All 26 formerly-monolithic tests pass with `assert_compiles_incremental()`
- **assert-compiles-migration.AC3.2 Success:** `assert_compiles()` method deleted from `test_common.rs`
- **assert-compiles-migration.AC3.3 Success:** `compile()` method deleted from `test_common.rs` (if no other callers)
- **assert-compiles-migration.AC3.4 Success:** `cargo test -p simlin-engine` passes with zero regressions

## Glossary

- **Incremental compilation / salsa**: The engine's current compilation strategy, where each variable is compiled independently and results are cached using salsa (a Rust framework for demand-driven incremental computation). Only variables whose inputs change are recompiled.
- **Monolithic compilation**: The older compilation path that compiles an entire project in one pass without caching. It is the code being removed by this migration.
- **AST lowering chain (Expr0 -> Expr1 -> Expr2 -> Expr3 -> Expr)**: The sequence of intermediate expression representations that an equation passes through during compilation. Each stage resolves more syntactic sugar and subscript information until the final `Expr` form is ready for code generation.
- **@N position syntax / DimPosition**: A subscript notation (e.g., `@1`, `@3`) that refers to the Nth element of a dimension by numeric position rather than by name. `DimPosition` is the corresponding AST node.
- **A2A (apply-to-all)**: A system dynamics concept where a single equation defines all elements of an arrayed variable. In the compiler, A2A context is tracked via `active_dimension`.
- **Scalar context**: The opposite of A2A context -- when the left-hand side of an equation is a plain (non-arrayed) variable, so there is no active dimension to iterate over.
- **Array-reduce builtins**: Built-in functions (SUM, SIZE, STDDEV, VMIN, VMAX, MEAN) that collapse an array argument into a single scalar value.
- **`walk_expr_as_view` / `PopView`**: Internal codegen methods that set up and tear down a "view" -- a runtime slice of an array -- so that an array opcode can operate over the selected elements.
- **`ViewRangeDynamic`**: A VM opcode that constructs an array view whose bounds are determined at runtime (from variable values) rather than at compile time.
- **`lower_index_expr3`**: The function in `context.rs` responsible for resolving subscript expressions during the Expr3 lowering stage, including dimension positions and dynamic subscripts.
- **`codegen.rs`**: The compiler module that translates the final AST (`Expr`) into stack-based bytecode instructions for the VM.

## Architecture

The migration has three independent workstreams that converge on deleting `assert_compiles()`:

1. **Trivial test switch (23 tests):** `builtins_visitor.rs` (20), `db_tests.rs` (2), `db_prev_init_tests.rs` (1) -- change `assert_compiles()` to `assert_compiles_incremental()` with no compiler changes.

2. **@N position syntax in scalar context:** When `arr[@1]` appears with a scalar LHS (no active A2A dimension), `lower_index_expr3` in `context.rs` currently returns `ArrayReferenceNeedsExplicitSubscripts`. The fix resolves `DimPosition(n)` to a concrete element name early in the lowering pipeline: look up the referenced dimension's element list and replace `@N` with `elements[n-1]`. No VM changes or new opcodes needed. A2A context (`matrix[@2, @1]`) already works via the existing dynamic subscript path.

3. **MEAN with dynamic ranges + array-reduce abstraction:** The `BuiltinFn::Mean` handler in `codegen.rs` gates the array path on `matches!(arg, Expr::StaticSubscript | Expr::TempArray)`, missing `Expr::Subscript` with `Range` indices. SUM, SIZE, and STDDEV work because they call `walk_expr_as_view` unconditionally. The fix introduces a shared `emit_array_reduce` helper and a robust `is_array_expr` check, eliminating the 6x copy-paste of `walk_expr_as_view + ArrayOp + PopView` across SUM, SIZE, STDDEV, VMIN, VMAX, and MEAN.

4. **Cleanup:** Delete `assert_compiles()` and its sole dependency `compile()` from `test_common.rs`. Verify no other callers exist before removing.

## Existing Patterns

The incremental compilation pipeline (`compile_var_fragment` in `db.rs`) compiles one variable at a time through the AST lowering chain: `Expr0 -> Expr1 -> Expr2 -> Expr3 -> Expr`. Subscript resolution happens in `context.rs` via `normalize_subscripts3` (static path producing `IndexOp`) and `lower_index_expr3` (dynamic fallback). The `@N` fix follows the existing pattern of resolving indices to concrete element names during lowering.

The array-reduce builtins (SUM, SIZE, STDDEV, VMIN, VMAX, MEAN) all share the same terminal pattern: `walk_expr_as_view(arg)` then emit an `Array*` opcode then `PopView`. This pattern is currently duplicated 6 times with no shared helper. The refactoring extracts this into a single method, which is a new pattern but one that follows the codebase's general preference for reducing duplication.

The `ViewRangeDynamic` opcode already exists in the VM and handles dynamic range bounds. The MEAN fix leverages this existing infrastructure.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: @N Position Syntax in Scalar Context

**Goal:** Make `arr[@N]` work on the incremental compilation path when the LHS is a scalar variable.

**Components:**
- `src/simlin-engine/src/compiler/context.rs` -- modify `lower_index_expr3` to resolve `IndexExpr3::DimPosition(pos)` to `IndexExpr3::Named(elements[pos-1])` when `active_dimension.is_none()`, with a compile error for out-of-range positions
- `src/simlin-engine/src/array_tests.rs` -- switch `dimension_position_single` (line 342) and `dimension_position_and_wildcard` (line 1366) from `assert_compiles()` to `assert_compiles_incremental()`

**Dependencies:** None

**Done when:** `dimension_position_single` and `dimension_position_and_wildcard` pass with `assert_compiles_incremental()`. Covers `assert-compiles-migration.AC1.*`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Array-Reduce Abstraction and MEAN Dynamic Ranges

**Goal:** Unify the array-reduce codegen pattern across all builtins and fix MEAN with dynamic range arguments.

**Components:**
- `src/simlin-engine/src/compiler/codegen.rs` -- extract `emit_array_reduce(arg, opcode)` helper; refactor SUM, SIZE, STDDEV, VMIN, VMAX, MEAN handlers to use it; fix MEAN's array detection to include `Expr::Subscript` with `Range` indices
- `src/simlin-engine/src/array_tests.rs` -- switch `mean_with_dynamic_range` (line 2023) from `assert_compiles()` to `assert_compiles_incremental()`

**Dependencies:** None (independent of Phase 1)

**Done when:** `mean_with_dynamic_range` passes with `assert_compiles_incremental()`. All existing array-reduce tests continue to pass. Covers `assert-compiles-migration.AC2.*`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Bulk Test Migration and Cleanup

**Goal:** Switch all remaining 23 tests and delete `assert_compiles()`.

**Components:**
- `src/simlin-engine/src/builtins_visitor.rs` -- switch 20 `assert_compiles()` calls to `assert_compiles_incremental()`
- `src/simlin-engine/src/db_tests.rs` -- switch 2 `assert_compiles()` calls to `assert_compiles_incremental()`
- `src/simlin-engine/src/db_prev_init_tests.rs` -- switch 1 `assert_compiles()` call to `assert_compiles_incremental()`
- `src/simlin-engine/src/test_common.rs` -- delete `assert_compiles()` method and `compile()` method (if no other callers remain)

**Dependencies:** Phases 1 and 2 (all tests must pass on incremental path first)

**Done when:** Zero calls to `assert_compiles()` remain in the codebase. `assert_compiles` and `compile()` are deleted from `test_common.rs`. All engine tests pass. Covers `assert-compiles-migration.AC3.*`.
<!-- END_PHASE_3 -->

## Additional Considerations

**Out-of-range @N:** If a user writes `arr[@5]` on a 3-element dimension, the compiler should emit a clear error at compile time rather than silently producing wrong results. This is a new error path that needs a test.

**Other builtins with dynamic ranges:** The array-reduce abstraction in Phase 2 should be verified against all builtins that accept array arguments, not just MEAN. If any other builtin has the same narrow `is_array` check, fix it in the same pass.
