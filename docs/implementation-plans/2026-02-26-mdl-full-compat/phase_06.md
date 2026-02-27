# MDL Full Compatibility -- Phase 6: Compiler-Level Array Operations

**Goal:** Implement VECTOR operations (SELECT, ELM MAP, SORT ORDER) and ALLOCATE AVAILABLE at the compiler level, and extend dimension mapping to handle cross-variable array assignments needed by mapping/multimap/subscript test models.

**Architecture:** Two tracks: (1) activate the existing scaffolded `DimensionMapping` infrastructure in `compiler/dimensions.rs` to resolve cross-dimension array assignments (currently `#[allow(dead_code)]`), and (2) add new `BuiltinFn` variants for VECTOR operations that compile to array iteration patterns using the existing view stack and `ArraySum`/`BeginIter`/`StoreIterElement` opcode infrastructure.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 7 phases from original design (phase 6 of 7)

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mdl-full-compat.AC5: Missing builtins
- **mdl-full-compat.AC5.4 Success:** VECTOR SELECT, VECTOR ELM MAP, VECTOR SORT ORDER, ALLOCATE AVAILABLE produce correct array outputs

---

## Reference Files

Compiler array infrastructure:
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/expr.rs` -- `Expr` enum (line 61), `decompose_array_temps` stub (line 224)
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/codegen.rs` -- `ArraySum` pattern (line 850), `AssignTemp` iteration (line 990)
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/dimensions.rs` -- `DimensionMapping` (line 18, dead_code), `broadcast_view` (line 170, dead_code), `match_dimensions_two_pass_partial` (line 79)
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/subscript.rs` -- `IndexOp`, `SparseRange` (line 15), `build_view_from_ops` (line 281)
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/context.rs` -- dimension mapping in A2A (line 197), `translate_to_source_via_mapping` usage
- `/home/bpowers/src/simlin/src/simlin-engine/src/ast/array_view.rs` -- `ArrayView` (line 26), `reorder_dimensions` (line 164)

Dimension infrastructure:
- `/home/bpowers/src/simlin/src/simlin-engine/src/dimensions.rs` -- `NamedDimension.maps_to` (line 21), `DimensionsContext` (line 255)

MDL recognition:
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/builtins.rs` -- "vector select", "vector elm map", etc. (lines 312-318)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/xmile_compat.rs` -- VECTOR function name mapping (lines 447-454), ALLOCATE argument reordering (line 332)

Test models:
- `test/sdeverywhere/models/mapping/` -- MismatchedDimensions (cross-dim assignment)
- `test/sdeverywhere/models/multimap/` -- MismatchedDimensions
- `test/sdeverywhere/models/subscript/` -- MismatchedDimensions
- `test/sdeverywhere/models/vector/` -- MismatchedDimensions (VECTOR ELM MAP)
- `test/sdeverywhere/models/allocate/` -- NotSimulatable (ALLOCATE AVAILABLE)

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
## Subcomponent A: Extended Dimension Mappings

<!-- START_TASK_1 -->
### Task 1: Activate dimension mapping infrastructure for cross-variable array assignment

**Verifies:** mdl-full-compat.AC5.4 (prerequisite for mapping/multimap/subscript)

**Files:**
- Modify: `src/simlin-engine/src/compiler/dimensions.rs` (remove `#[allow(dead_code)]`)
- Modify: `src/simlin-engine/src/compiler/context.rs` (integrate mapping into array assignment)
- Modify: `src/simlin-engine/src/compiler/codegen.rs` (use `broadcast_view` for dimension remapping)

**Implementation:**

The compiler scaffolding for dimension mapping already exists but is unused:
- `DimensionMapping` struct at `dimensions.rs:18`
- `broadcast_view` at `dimensions.rs:170`
- `find_dimension_reordering` at `dimensions.rs:227`

The current `MismatchedDimensions` error occurs when the compiler tries to assign `a[DimA]` to `b[DimB]` and the dimensions don't match by name. The `maps_to` path in `context.rs:197` handles A2A subscript resolution (looking up a specific element), but does NOT handle full array-to-array dimension remapping.

The fix:
1. When a `MismatchedDimensions` would be returned during array variable lowering, check if any source dimension has a `maps_to` that targets the active dimension (or vice versa)
2. If a mapping exists, use `broadcast_view` or `find_dimension_reordering` to create a remapped view
3. Integrate with the Phase 2 `DimensionMapping.element_map` for element-level correspondence

The existing `match_dimensions_two_pass_partial` at line 79 already does the matching logic -- wire it into the codegen path where dimension alignment is checked.

**Testing:**

Add unit tests using `TestProject` with two dimensions where one maps to the other, verifying that `a[DimA] = b[DimB]` compiles and simulates correctly when `DimA` maps to `DimB`.

**Verification:**
Run: `cargo test -p simlin-engine compiler`
Expected: All tests pass

**Commit:** `engine: activate dimension mapping for cross-variable array assignment`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Handle multi-dimension mappings from Phase 2 datamodel

**Verifies:** mdl-full-compat.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/dimensions.rs` (update for `mappings: Vec<DimensionMapping>`)
- Modify: `src/simlin-engine/src/compiler/context.rs`

**Implementation:**

After Phase 2 replaces `Dimension.maps_to` with `mappings: Vec<DimensionMapping>`, the compiler's `DimensionsContext` needs to handle:
1. Multiple mappings per dimension (one dim maps to several targets)
2. Element-level mappings (specific source elements map to specific target elements)

Update `DimensionsContext::get_maps_to()` and `translate_to_source_via_mapping()` to:
- Iterate `mappings` to find applicable mapping for a given target dimension
- For element-level mappings, use the `element_map: Vec<(String, String)>` to look up specific element correspondence instead of positional matching

**Testing:**

Add unit test with element-level dimension mapping (e.g., `DimA.A1` maps to `DimB.B3`) and verify correct compilation.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: support element-level dimension mappings in compiler`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Enable mapping, multimap, subscript test models

**Verifies:** mdl-full-compat.AC5.4

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (uncomment mapping, multimap, subscript)

**Implementation:**

Uncomment from `TEST_SDEVERYWHERE_MODELS`:
- `mapping/mapping.xmile`
- `multimap/multimap.xmile`
- `subscript/subscript.xmile`

Run and fix any remaining issues. The `subscript` model may exercise additional subscript features beyond dimension mapping.

**Testing:**

Run the test models and compare output against `.dat` expected results.

**Verification:**
Run: `cargo test --features file_io --test simulate -- mapping multimap subscript`
Expected: All three tests pass

**Commit:** `engine: enable mapping, multimap, subscript test models`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-9) -->
## Subcomponent B: VECTOR Operations and Array Expression Support

<!-- START_TASK_4 -->
### Task 4: Implement VECTOR SELECT

**Verifies:** mdl-full-compat.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs` (add VectorSelect variant)
- Modify: `src/simlin-engine/src/compiler/codegen.rs` (compilation to opcodes)
- Modify: `src/simlin-engine/src/interpreter.rs` (interpreter implementation)
- Modify: `src/simlin-engine/src/compiler/expr.rs` (strip_loc, map methods)

**Implementation:**

`VECTOR SELECT(selection_array, expression_array, max_value, missing_value, action)` selects elements from `expression_array` where `selection_array` is nonzero, then applies `action` (SUM, MIN, MAX, etc.) to the selected elements.

Add `VectorSelect` to `BuiltinFn`:
```rust
VectorSelect(Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>, Box<Expr>),
```

For compilation, VECTOR SELECT is equivalent to:
1. Iterate over all elements of the selection array
2. For each element where selection != 0, include the corresponding expression_array element
3. Apply the action function (SUM/MIN/MAX) to the included elements

Use the existing `BeginIter`/`StoreIterElement`/`NextIterOrJump` pattern with conditional element inclusion.

For the interpreter, implement directly as an array iteration with conditional reduction.

**Testing:**

Add unit tests with `TestProject`:
- VECTOR SELECT with SUM action
- VECTOR SELECT with all zeros (returns missing_value)
- VECTOR SELECT with partial selection

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: implement VECTOR SELECT array operation`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Implement VECTOR ELM MAP and VECTOR SORT ORDER

**Verifies:** mdl-full-compat.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs`
- Modify: `src/simlin-engine/src/compiler/codegen.rs`
- Modify: `src/simlin-engine/src/interpreter.rs`
- Modify: `src/simlin-engine/src/compiler/expr.rs`

**Implementation:**

**VECTOR ELM MAP(source_array, offset_array)**: For each element, uses the value in `offset_array` as an index into `source_array`. Essentially `result[i] = source_array[offset_array[i]]`.

Add `VectorElmMap(Box<Expr>, Box<Expr>)` to `BuiltinFn`. Compilation:
1. Push source_array and offset_array views
2. For each element of the output, load offset_array[i], use as index into source_array
3. Store result

**VECTOR SORT ORDER(array, direction)**: Returns an array of indices that would sort the input array. `direction` = 1 for ascending, -1 for descending.

Add `VectorSortOrder(Box<Expr>, Box<Expr>)` to `BuiltinFn`. This may need VM support for comparison-based sorting since the sort itself is complex. Consider:
- Interpreter: direct sort using Rust's `sort_by` with index tracking
- VM: emit a `VectorSort` opcode that operates on the view stack

**Testing:**

Add unit tests:
- VECTOR ELM MAP with known offset mapping
- VECTOR SORT ORDER ascending and descending

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: implement VECTOR ELM MAP and VECTOR SORT ORDER`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Implement ALLOCATE AVAILABLE

**Verifies:** mdl-full-compat.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs`
- Modify: `src/simlin-engine/src/compiler/codegen.rs` or create stdlib model
- Modify: `src/simlin-engine/src/interpreter.rs`

**Implementation:**

`ALLOCATE AVAILABLE(request, priority, width, supply)` performs priority-based allocation of a scarce resource across array elements. Elements with higher priority get allocated first, up to their request amount, until supply is exhausted.

The design notes this is the most complex single builtin. Two approaches:

**Approach A (compiler-level):** Generate specialized iteration code:
1. Sort elements by priority (descending)
2. Iterate in priority order, allocating min(request[i], remaining_supply) to each
3. Return the allocation array

**Approach B (stdlib model):** Create a stock-flow model that accumulates allocations. This may be unwieldy for an array operation.

**Approach C (VM opcode):** Add an `AllocateAvailable` opcode that takes array views and performs the allocation in a single VM step.

Recommend Approach C or A depending on whether the iteration pattern fits existing infrastructure. The `width` parameter adds smoothing around the priority cutoff.

The xmile_compat.rs already handles ALLOCATE argument reordering (line 332). Ensure the compiled form matches the argument order.

**Testing:**

Add unit tests:
- ALLOCATE with supply exceeding total demand (all requests fulfilled)
- ALLOCATE with supply less than total demand (priority-based partial allocation)
- ALLOCATE with width smoothing

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: implement ALLOCATE AVAILABLE`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_7 -->
### Task 7: Support If expressions inside array iteration (SUM of conditional)

**Verifies:** mdl-full-compat.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/compiler/codegen.rs` (around `walk_expr_as_view`, line 283)

**Implementation:**

The `sumif` test model uses `SUM(IF THEN ELSE(A_Values[*]=:NA:, 0, A_Values[*]))` -- a standard SUM containing a conditional expression. Currently `walk_expr_as_view` in `codegen.rs` only handles `StaticSubscript`, `TempArray`, `Var`, and `Subscript` variants. When an `If` expression appears inside SUM, the codegen fails with "Cannot push view for expression type Discriminant(12)".

The fix: extend `walk_expr_as_view` (or the array iteration path that calls it) to handle `If` expressions by decomposing them into element-wise conditional evaluation. When SUM encounters an `If` expression with array subscripts, it should:
1. Iterate over the array dimension
2. For each element, evaluate the condition, then/else branches
3. Accumulate the selected branch values

This can reuse the existing `BeginIter`/`NextIterOrJump` pattern, evaluating the `If` per-element within the iteration body.

**Testing:**

Add a unit test using `TestProject` with `SUM(IF a[*] > 0 THEN a[*] ELSE 0)` pattern.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: support If expressions inside array iteration contexts`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Enable vector, allocate, and sumif test models

**Verifies:** mdl-full-compat.AC5.4

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs`

**Implementation:**

Uncomment from `TEST_SDEVERYWHERE_MODELS`:
- `vector/vector.xmile`
- `allocate/allocate.xmile`
- `sumif/sumif.xmile`

Run and verify simulation output matches expected `.dat` files.

**Verification:**
Run: `cargo test --features file_io --test simulate -- vector allocate sumif`
Expected: All tests pass

Run: `cargo test --features file_io --test simulate`
Expected: All simulation tests pass (no regressions)

**Commit:** `engine: enable vector, allocate, and sumif test models`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Final verification

**Verifies:** mdl-full-compat.AC5.4

**Files:** None (verification only)

**Implementation:**

Run the full test suite:

```bash
cargo test -p simlin-engine
cargo test --features file_io --test simulate
```

Verify:
- All mapping/multimap/subscript models pass
- All vector models pass
- All allocate models pass
- sumif model passes
- No regressions in existing tests

No commit (verification only).
<!-- END_TASK_9 -->
