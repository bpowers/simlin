# Close Array Gaps Implementation Plan -- Phase 5

**Goal:** Wire arrayed GET DIRECT resolution into the MDL converter pipeline (unblocking 3 of 4 blocked test models) and implement the RANK builtin end-to-end.

**Architecture:** Two independent workstreams: (1) extend the MDL converter's `try_resolve_data_expr` to handle arrayed GET DIRECT patterns (star-pattern constants, 2D grids, arrayed lookups) by iterating over the existing single-cell `DataProvider` methods; (2) implement RANK end-to-end through MDL parser recognition, xmile_compat rename, compiler codegen, VM opcode, and interpreter.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 6 phases from original design (this is phase 5 of 6)

**Codebase verified:** 2026-03-11

**Testing references:** See `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` for test index.

---

## Acceptance Criteria Coverage

This phase implements and tests:

### close-array-gaps.AC7: GET DIRECT wiring (#348)
- **close-array-gaps.AC7.1 Success:** `simulates_directsubs_mdl` passes (GET DIRECT SUBSCRIPT in dimension definitions)
- **close-array-gaps.AC7.2 Success:** `simulates_directconst_mdl` passes (arrayed GET DIRECT CONSTANTS with star pattern + 2D grid)
- **close-array-gaps.AC7.3 Success:** `simulates_directlookups_mdl` passes (arrayed GET DIRECT LOOKUPS)
- **close-array-gaps.AC7.4 Edge:** `simulates_directdata_mdl` remains ignored (ext_data feature, out of scope)

### close-array-gaps.AC8: RANK builtin (#359)
- **close-array-gaps.AC8.1 Success:** MDL parser recognizes `VECTOR RANK` and maps to `Rank` builtin
- **close-array-gaps.AC8.2 Success:** `RANK(A, N)` returns value at 1-based position N of sorted array in both VM and interpreter
- **close-array-gaps.AC8.3 Success:** `RANK(A, N, B)` with tie-break array works correctly
- **close-array-gaps.AC8.4 Success:** Unit tests cover 1-arg, 2-arg, and 3-arg forms

---

## Codebase Verification Findings

- Confirmed: `DataProvider` trait at `data_provider/mod.rs:16-75` has 4 methods: `load_data`, `load_constant`, `load_lookup`, `load_subscript`. All exist and are implemented in `csv_provider.rs`.
- Discrepancy: Design proposed `load_constants_range`, `load_constants_grid`, `load_lookups_array` methods on the trait. These do NOT exist. The current architecture uses single-cell access; arrayed resolution requires caller-side iteration in the MDL converter, not new trait methods.
- Confirmed: GET DIRECT SUBSCRIPT is fully wired into dimension building at `convert/dimensions.rs:244-311`.
- Confirmed: `try_resolve_data_expr` (not `try_resolve_data_equation`) at `convert/external_data.rs:289`. Scalar path works end-to-end.
- Confirmed: `BuiltinFn::Rank(Box<Expr>, Option<(Box<Expr>, Option<Box<Expr>>)>)` exists at `builtins.rs:93`.
- Missing: `"vector rank"` NOT in `mdl/builtins.rs` BUILTINS set -- MDL models using VECTOR RANK fail to recognize the builtin.
- Missing: No `vector_rank` -> `rank` rename in `xmile_compat.rs:494-533`.
- Confirmed: `"rank"` recognized in XMILE AST lowering at `ast/expr1.rs:231`.
- Confirmed: Compiler codegen returns `TodoArrayBuiltin` for Rank at `codegen.rs:883-885,956-958`.
- Confirmed: No `Rank` opcode in `bytecode.rs`. No VM implementation.
- Confirmed: Interpreter has `unreachable!()` for Rank at `interpreter.rs:1251-1253`.
- Confirmed: All four `directXxx_mdl` tests in `simulate.rs` are `#[ignore]`.
- Confirmed: `simulates_directdata_mdl` additionally requires `ext_data` feature (out of scope).

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Wire arrayed GET DIRECT CONSTANTS and LOOKUPS resolution

**Verifies:** close-array-gaps.AC7.2, close-array-gaps.AC7.3

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/external_data.rs` (try_resolve_data_expr and resolve_get_direct)
- Modify: `src/simlin-engine/src/mdl/convert/variables.rs` (call site for data equation resolution)

**Implementation:**

The existing scalar GET DIRECT path resolves one value per call. For arrayed patterns, the MDL converter needs to iterate:

1. **Arrayed GET DIRECT CONSTANTS (`B2*` star pattern):** When `resolve_get_direct` encounters a star in the cell reference (e.g., `B2*`), it should iterate over the target dimension elements, calling `provider.load_constant` for each element's row/column position. Produce an `Equation::Arrayed` with per-element constant values instead of a single scalar.

2. **2D grid constants:** When both row and column use dimension references, iterate over the cartesian product of two dimensions. Produce a 2D `Equation::Arrayed`.

3. **Arrayed GET DIRECT LOOKUPS:** Similar to constants -- iterate over the target dimension elements, calling `provider.load_lookup` for each element. Produce an `Equation::Arrayed` with per-element graphical functions.

The key is extending `try_resolve_data_expr` to detect when the calling variable is arrayed (has dimensions) and the GET DIRECT reference uses a star/range pattern, then producing the appropriate `Equation::Arrayed` with per-element data.

Study the `directconst.mdl` and `directlookups.mdl` test models to understand the exact patterns that need handling.

**Testing:**

Tests in Task 3 via integration tests.

**Verification:**

```bash
cargo build -p simlin-engine --features file_io
```

Expected: Compiles without errors.

**Commit:** `engine: wire arrayed GET DIRECT CONSTANTS and LOOKUPS resolution`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Fix GET DIRECT SUBSCRIPT for cross-dimension test model

**Verifies:** close-array-gaps.AC7.1

**Files:**
- Modify: `src/simlin-engine/src/mdl/convert/dimensions.rs` (if needed for directsubs model)
- Modify: `src/simlin-engine/src/mdl/convert/external_data.rs` (if needed)

**Implementation:**

The GET DIRECT SUBSCRIPT path is already wired at `convert/dimensions.rs:244-311`. The `simulates_directsubs_mdl` test may be failing due to cross-dimension mapping issues (the ignore comment mentions "DimA -> DimB, DimC") rather than missing GET DIRECT SUBSCRIPT support.

Study the `directsubs.mdl` test model to understand what's blocking it. The fix may involve:
- Ensuring dimension mappings from Phase 2/4 work with GET DIRECT SUBSCRIPT-defined dimensions
- Fixing any remaining dimension resolution issues

If the test already passes after Phases 2-4 without additional changes, simply un-ignore it.

**Testing:**

Tests in Task 3.

**Verification:**

```bash
cargo test -p simlin-engine --features file_io,testing --test simulate simulates_directsubs_mdl -- --ignored
```

Expected: Test passes (or identifies remaining issues to fix).

**Commit:** `engine: fix GET DIRECT SUBSCRIPT for cross-dimension models` (or fold into Task 3 if no code changes needed)
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Un-ignore GET DIRECT integration tests

**Verifies:** close-array-gaps.AC7.1, close-array-gaps.AC7.2, close-array-gaps.AC7.3, close-array-gaps.AC7.4

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:1290-1313` (GET DIRECT test ignore annotations)

**Implementation:**

1. Remove `#[ignore]` from:
   - `simulates_directsubs_mdl` (line 1311)
   - `simulates_directconst_mdl` (line 1297)
   - `simulates_directlookups_mdl` (line 1304)

2. Keep `#[ignore]` and `#[cfg(feature = "ext_data")]` on `simulates_directdata_mdl` (line 1290) -- requires Excel support, out of scope.

3. Remove or update the explanatory comments above each test.

**Testing:**

- close-array-gaps.AC7.1: `simulates_directsubs_mdl` passes
- close-array-gaps.AC7.2: `simulates_directconst_mdl` passes
- close-array-gaps.AC7.3: `simulates_directlookups_mdl` passes
- close-array-gaps.AC7.4: `simulates_directdata_mdl` remains ignored

**Verification:**

```bash
cargo test -p simlin-engine --features file_io,testing --test simulate simulates_direct
```

Expected: directsubs, directconst, directlookups pass. directdata remains ignored.

**Commit:** `engine: un-ignore GET DIRECT integration tests`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-7) -->
<!-- START_TASK_4 -->
### Task 4: Add VECTOR RANK to MDL parser and xmile_compat rename

**Verifies:** close-array-gaps.AC8.1

**Files:**
- Modify: `src/simlin-engine/src/mdl/builtins.rs:248-342` (BUILTINS set)
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs:494-533` (format_function_name)

**Implementation:**

1. Add `"vector rank"` to the `BUILTINS` HashSet in `mdl/builtins.rs`. This allows the MDL classifier to recognize `VECTOR RANK` as a builtin function (after Vensim name normalization converts it to lowercase `vector rank`).

2. Add a rename mapping in `xmile_compat.rs` `format_function_name`:
```rust
"vector rank" => "rank".to_string(),
```

This ensures that when MDL equations are converted to the engine's AST representation (via XMILE-compatible names), `VECTOR RANK` maps to the existing `"rank"` builtin recognized by `expr1.rs:231`.

**Testing:**

- close-array-gaps.AC8.1: Add a test that parses an MDL equation containing `VECTOR RANK(...)` and verifies it produces a `BuiltinFn::Rank` AST node.

**Verification:**

```bash
cargo test -p simlin-engine
```

Expected: All existing tests pass, new parser test passes.

**Commit:** `engine: add VECTOR RANK to MDL parser and xmile_compat rename`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Implement RANK in interpreter

**Verifies:** close-array-gaps.AC8.2, close-array-gaps.AC8.3

**Files:**
- Modify: `src/simlin-engine/src/interpreter.rs:1251-1253` (replace unreachable! with implementation)

**Implementation:**

Replace the `unreachable!()` for `BuiltinFn::Rank` with the actual implementation:

`RANK(A, N)` semantics (Vensim): Given array A and 1-based position N, sort A in ascending order and return the value at position N.

`RANK(A, N, B)` semantics: Same as above, but when elements of A are tied, use array B as a tiebreaker (secondary sort key).

**Important:** Before implementing, validate exact RANK semantics against the Vensim reference documentation and the test model expected outputs. The description above is approximate -- Vensim's RANK may differ in sort direction, tie-breaking conventions, or edge case handling. Use the test model `.dat` files as the authoritative specification for expected behavior.

Implementation approach:
1. Collect all elements of array A (using `iter_array_elements`)
2. Sort ascending (with optional secondary sort on B)
3. Return the element at 1-based position N (return NaN if N is out of bounds)

The `BuiltinFn::Rank` variant is `Rank(Box<Expr>, Option<(Box<Expr>, Option<Box<Expr>>)>)`:
- First arg: array expression A
- Second arg (inside Option tuple): position N
- Third arg (inside Option tuple's Option): tiebreak array B

**Testing:**

Tests in Task 7.

**Verification:**

```bash
cargo build -p simlin-engine
```

Expected: Compiles.

**Commit:** `engine: implement RANK builtin in interpreter`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Implement RANK in compiler and VM

**Verifies:** close-array-gaps.AC8.2, close-array-gaps.AC8.3

**Files:**
- Modify: `src/simlin-engine/src/bytecode.rs` (add Rank opcode)
- Modify: `src/simlin-engine/src/compiler/codegen.rs:883-885,956-958` (replace TodoArrayBuiltin with opcode emission)
- Modify: `src/simlin-engine/src/vm.rs` (implement Rank opcode execution)

**Implementation:**

1. **bytecode.rs:** Add `Rank {}` variant to the `Opcode` enum. RANK operates on array views (like other array builtins), taking the array view from the view stack, position N from the value stack, and optionally tiebreak array B from the view stack.

2. **codegen.rs:** Replace the two `TodoArrayBuiltin` returns for Rank (lines 883-885 and 956-958) with proper opcode emission. Follow the pattern used by other array builtins in `emit_array_reduce`:
   - Push the source array view onto the view stack
   - Push the position argument onto the value stack
   - If tiebreak array B exists, push its view onto the view stack
   - Emit `Opcode::Rank`
   - Pop views after execution

3. **vm.rs:** Implement `Opcode::Rank` execution:
   - Pop the source array view
   - Pop position N from the value stack
   - Collect all elements from the view, sort ascending
   - If tiebreak view is present, use it for secondary sorting
   - Return the value at 1-based position N (NaN for out-of-bounds)

**Testing:**

Tests in Task 7.

**Verification:**

```bash
cargo build -p simlin-engine
```

Expected: Compiles.

**Commit:** `engine: implement RANK opcode in compiler and VM`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Add RANK unit tests

**Verifies:** close-array-gaps.AC8.2, close-array-gaps.AC8.3, close-array-gaps.AC8.4

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs` (add RANK test module)

**Implementation:**

Add tests using `TestProject` builder that verify RANK in both interpreter and VM:

1. **2-arg form `RANK(A, N)`:** Create array `A[Dim] = {30, 10, 20}` and `result = RANK(A, 2)`. Sorted ascending: [10, 20, 30]. Position 2 = 20. Verify `result = 20` via both `assert_interpreter_result` and `assert_vm_result_incremental`.

2. **1-arg form `RANK(A, N)` with varying N:** Create arrayed `result[Dim] = RANK(A, Dim)`. Verify `result = [10, 20, 30]` (sorted ascending, each position returns the Nth value).

3. **3-arg form `RANK(A, N, B)` with tiebreak:** Create `A[Dim] = {10, 10, 20}` and `B[Dim] = {2, 1, 3}`. Position 1 should return the tied A=10 element with lower B value (B=1, so A[Dim2]). Position 2 returns A=10 with B=2. Verify via both paths.

4. **Out-of-bounds:** `RANK(A, 0)` or `RANK(A, 4)` for 3-element array should return NaN. Use `interpreter_result`/`vm_result_incremental` and check with `f64::is_nan()`.

**Testing:**

- close-array-gaps.AC8.2: Tests 1 and 2 verify basic RANK behavior
- close-array-gaps.AC8.3: Test 3 verifies tiebreak behavior
- close-array-gaps.AC8.4: All four tests cover 1-arg (position from dimension), 2-arg, and 3-arg forms

**Verification:**

```bash
cargo test -p simlin-engine array_tests
```

Expected: All RANK tests pass in both interpreter and VM.

**Commit:** `engine: add RANK builtin unit tests`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_B -->
