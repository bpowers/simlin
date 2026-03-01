# VM Vector Operations Design

## Summary

The simulation engine contains two evaluation paths: a tree-walking interpreter that serves as the reference implementation, and a bytecode VM that runs compiled models faster. For most operations, both paths produce identical results. Four array-oriented Vensim/Stella built-in functions -- VECTOR SELECT, VECTOR ELM MAP, VECTOR SORT ORDER, and ALLOCATE AVAILABLE -- are currently supported only by the interpreter; the VM returns an unimplemented error. This design describes adding VM support for all four.

The complexity comes from a structural difference between these operations and the scalar builtins already supported. The engine handles arrayed variables by applying equations element-by-element using apply-to-all (A2A) expansion at compile time -- each array element gets its own assignment. Three of the four builtins (ELM MAP, SORT ORDER, ALLOCATE AVAILABLE) need to see the entire input array at once to produce their output; they cannot be decomposed into independent per-element computations. The approach is to detect these builtins during A2A expansion and hoist them into a single pre-computation step (`AssignTemp`) that writes the full output array into pre-allocated `temp_storage`, after which per-element reads pull individual results from that region. This same compiler change simultaneously fixes a confirmed bug in the interpreter where these builtins incorrectly return the first element's value for every element of the array. VECTOR SELECT is simpler -- it reduces an array to a single scalar, following the same stack-based reduction pattern already used by `ArraySum`, `ArrayMax`, and similar operations.

## Definition of Done

1. **Four new VM opcodes** (VECTOR SELECT, VECTOR ELM MAP, VECTOR SORT ORDER, ALLOCATE AVAILABLE) with compiler emission in codegen.rs and VM dispatch in vm.rs that produce identical results to the interpreter for all inputs.
2. **Minimal allocations** -- SmallVec for scratch buffers (stack-allocated for typical SD model sizes of 1-32 elements), pre-allocated temp_storage for array outputs. No heap allocation in the common case.
3. **Test upgrades** -- the three `_interpreter_only` tests (`simulates_vector_simple_mdl`, `simulates_allocate_mdl`, `simulates_allocate_xmile`) run through the full VM+interpreter path and pass, confirming parity.
4. **Interpreter bug fix** -- the compiler change (A2A decomposition of array-producing builtins into AssignTemp) simultaneously fixes a confirmed interpreter bug where VectorSortOrder, VectorElmMap, and AllocateAvailable return the first element's result for every A2A element.
5. **Out of scope**: RANK (#359), cross-dimension VECTOR ELM MAP (#357), vector.xmile enablement (#358).

## Acceptance Criteria

### vm-vector-ops.AC1: VM produces correct results for all four operations
- **vm-vector-ops.AC1.1 Success:** VECTOR SELECT with VSSUM action returns sum of selected elements
- **vm-vector-ops.AC1.2 Success:** VECTOR SELECT with VSMIN/VSMEAN/VSMAX/VSPROD actions return correct reductions
- **vm-vector-ops.AC1.3 Success:** VECTOR SELECT with no elements selected returns max_value
- **vm-vector-ops.AC1.4 Success:** VECTOR ELM MAP returns source[round(offset[i])] for each element
- **vm-vector-ops.AC1.5 Failure:** VECTOR ELM MAP returns NaN for out-of-bounds offset
- **vm-vector-ops.AC1.6 Success:** VECTOR SORT ORDER ascending returns correct 1-based permutation indices
- **vm-vector-ops.AC1.7 Success:** VECTOR SORT ORDER descending returns correct 1-based permutation indices
- **vm-vector-ops.AC1.8 Success:** ALLOCATE AVAILABLE distributes supply according to priority profiles and bisection algorithm

### vm-vector-ops.AC2: Minimal allocations
- **vm-vector-ops.AC2.1 Success:** No heap allocation for arrays with <=32 elements (SmallVec inline capacity)
- **vm-vector-ops.AC2.2 Success:** Array-producing opcodes write to pre-allocated temp_storage, not new allocations

### vm-vector-ops.AC3: Test parity
- **vm-vector-ops.AC3.1 Success:** `simulates_vector_simple_mdl` passes with VM+interpreter comparison
- **vm-vector-ops.AC3.2 Success:** `simulates_allocate_mdl` passes with VM+interpreter comparison
- **vm-vector-ops.AC3.3 Success:** `simulates_allocate_xmile` passes with VM+interpreter comparison
- **vm-vector-ops.AC3.4 Success:** Expected output .dat file validates VectorElmMap and VectorSortOrder values against known-correct results

### vm-vector-ops.AC4: Interpreter bug fix
- **vm-vector-ops.AC4.1 Success:** VectorSortOrder in A2A context produces distinct per-element values (not all identical)
- **vm-vector-ops.AC4.2 Success:** VectorElmMap in A2A context produces correct per-element indirect lookups
- **vm-vector-ops.AC4.3 Success:** AllocateAvailable in A2A context produces correct per-requester allocations

## Glossary

- **A2A (Apply-to-All)**: The compiler strategy for arrayed variables. An equation defined over a dimension (e.g., `sales[region]`) is expanded into one assignment per element at compile time, each referencing a specific index.
- **AssignTemp**: A compiler IR node that hoists a subexpression into a named scratch slot. Used when a whole-array computation must happen once before per-element reads distribute the results.
- **ALLOCATE AVAILABLE**: A market-clearing allocation builtin. Given a total supply, a set of requests, and priority profiles, it distributes supply using a bisection algorithm over a normal-CDF priority function.
- **bisection algorithm**: A root-finding method that iteratively halves an interval to converge on a value satisfying a condition. Used here to find the priority threshold that exactly exhausts available supply.
- **Interpreter**: The AST-walking evaluator in `interpreter.rs`. It traverses the expression tree at runtime and is used as a correctness reference when verifying VM output.
- **normal CDF / erfc**: The cumulative distribution function of the standard normal distribution, approximated via the complementary error function. Used to shape the priority profile curves in ALLOCATE AVAILABLE.
- **RuntimeView**: The VM's handle to an array slice -- either a contiguous region of the current variable state (`curr[]`) or a range within `temp_storage`. Opcodes operate on views rather than raw pointers.
- **SmallVec**: A Rust library type that stores up to N elements on the stack (no heap allocation) and falls back to heap when the count exceeds N. Used throughout the VM to avoid allocation for small arrays typical in SD models.
- **stable sort**: A sort algorithm that preserves the original relative order of equal elements. Relevant to VECTOR SORT ORDER, where Rust's `sort_by` (stable) matches the interpreter's behavior for ties.
- **temp_storage**: The runtime scratch region where whole-array intermediate results live. Slots are allocated at compile time; the VM zeroes and reuses the same memory each timestep.
- **VECTOR ELM MAP**: A builtin for indirect array indexing -- element `i` of the output is `source[round(offset[i])]`. Requires the whole source array to be visible at once.
- **VECTOR SELECT**: A builtin that filters an array by a boolean selection mask and reduces the surviving elements (sum, min, mean, max, or product) to a single scalar.
- **VECTOR SORT ORDER**: A builtin that returns the 1-based permutation indices that would sort the input array (ascending or descending), analogous to `argsort` in NumPy.
- **VM (Virtual Machine)**: The bytecode executor in `vm.rs`. Models are compiled to an instruction sequence once; the VM steps through those instructions at each simulation timestep.

## Architecture

Four operations need VM support. They fall into two categories:

**Scalar-producing (reduction):** VECTOR SELECT filters an array by a selection mask, then reduces the selected elements (SUM/MIN/MEAN/MAX/PROD). Produces one scalar value.

**Array-producing (whole-array):** VECTOR ELM MAP (indirect array indexing), VECTOR SORT ORDER (sort permutation indices), and ALLOCATE AVAILABLE (market-clearing allocation) each produce an output array. These are inherently cross-element operations that cannot be evaluated one element at a time.

### Data flow

For array-producing operations, the pipeline is:

1. **Compiler A2A expansion** (`compiler/mod.rs`): Detects array-producing builtins in apply-to-all equations. Creates an `AssignTemp` pre-computation that evaluates the full array once, then replaces per-element references with `TempArrayElement` reads.
2. **Codegen** (`compiler/codegen.rs`): When the `AssignTemp` handler encounters one of these builtins as its RHS, it emits a dedicated whole-array opcode instead of the normal `BeginIter` iteration loop.
3. **VM dispatch** (`vm.rs`): The opcode reads input arrays from the view stack and scalar parameters from the arithmetic stack, computes the result, and writes it to pre-allocated `temp_storage`.
4. **Per-element reads**: Subsequent `TempArrayElement` references load individual elements from `temp_storage` via `LoadTempConst`/`LoadTempDynamic` opcodes (already supported).

For VECTOR SELECT (scalar-producing), the flow is simpler: the codegen emits the opcode inline in `walk_expr`, and the VM pushes one scalar result onto the arithmetic stack.

### Shared helpers

The allocation algorithm functions (`alloc_curve`, `allocate_available`, `normal_cdf`, `erfc_approx`) move from `interpreter.rs` to a new shared module `src/simlin-engine/src/alloc.rs`, callable by both the interpreter and VM.

A `read_view_element` helper in `vm.rs` extracts a single element from a `RuntimeView` at a given flat index, handling both `curr[]` and `temp_storage` sources. Used by VectorElmMap and VectorSelect.

### Opcodes

| Opcode | Inputs | Output | Scratch |
|--------|--------|--------|---------|
| `VectorSelect {}` | 2 views (selection, expression) + 2 scalars (max_value, action) | 1 scalar on arithmetic stack | SmallVec<[u16; 4]> for indices |
| `VectorElmMap { write_temp_id }` | 2 views (source, offset) | Array in temp_storage | None |
| `VectorSortOrder { write_temp_id }` | 1 view (input) + 1 scalar (direction) | Array in temp_storage | SmallVec<[(f64, u16); 32]> |
| `AllocateAvailable { write_temp_id }` | 2 views (requests, profile) + 1 scalar (avail) | Array in temp_storage | SmallVec<[f64; 32]>, SmallVec<[(f64,f64,f64,f64); 32]> |

### Interpreter bug fix

The compiler-level A2A decomposition simultaneously fixes a confirmed interpreter bug. Currently, the A2A expansion generates identical `AssignCurr(off+i, App(VectorSortOrder(full_view, dir)))` expressions for each element. The interpreter's `eval()` always returns the first element's result (e.g., `indexed[0].0` for sort), giving all elements the same value. The correct `eval_at_index()` implementation exists but is only called from the `AssignTemp` path. By routing these builtins through `AssignTemp`, the existing correct code path is activated.

## Existing Patterns

### Array reduction opcodes

The existing `ArraySum`, `ArrayMax`, `ArrayMin`, `ArrayMean`, `ArrayStddev`, `ArraySize` opcodes follow a consistent pattern: `walk_expr_as_view(arg)` pushes a view, the opcode reduces it to a scalar via `reduce_view()`, then `PopView`. VECTOR SELECT follows this same pattern but with two views and a selection filter.

### AssignTemp + BeginIter for array-producing expressions

The compiler already decomposes complex array subexpressions (Op2/Op1/If with array bounds) into `AssignTemp` via `needs_decomposition()` in `ast/expr3.rs`. The codegen emits `BeginIter`/`StoreIterElement`/`EndIter` loops. The new array-producing builtins reuse the `AssignTemp` wrapper but bypass `BeginIter` in favor of dedicated whole-array opcodes, since these operations cannot be decomposed into independent per-element computations.

### SmallVec for allocation avoidance

The VM uses `SmallVec<[u16; 4]>` throughout for dimension indices, `SmallVec<[f64; 16]>` for module inputs, and `SmallVec<[i8; 4]>` for broadcast mappings. The new opcodes follow the same pattern with inline capacities sized for typical SD models (1-32 elements).

### temp_storage lifecycle

`temp_storage` is allocated once in `Vm::new()`, zeroed on reset, and reused across timesteps. Temp IDs and sizes are determined at compile time via `extract_temp_sizes()`. The new opcodes write to the same pre-allocated temp slots.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Shared allocation helpers

**Goal:** Extract allocation math from interpreter into a shared module.

**Components:**
- New `src/simlin-engine/src/alloc.rs` -- contains `erfc_approx`, `normal_cdf`, `alloc_curve`, `allocate_available` functions (moved from `src/simlin-engine/src/interpreter.rs` lines 80-250)
- `src/simlin-engine/src/interpreter.rs` -- imports from `alloc.rs` instead of defining locally
- `src/simlin-engine/src/lib.rs` -- declares the new module

**Dependencies:** None (first phase)

**Done when:** `cargo test -p simlin-engine` passes with the helpers moved. No behavioral change.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Compiler A2A decomposition

**Goal:** Array-producing builtins (VectorElmMap, VectorSortOrder, AllocateAvailable) are decomposed into `AssignTemp` + `TempArrayElement` during A2A expansion. This fixes the interpreter bug and prepares the expression tree for VM codegen.

**Components:**
- `src/simlin-engine/src/compiler/mod.rs` -- modify `Ast::ApplyToAll` handler to detect array-producing builtins, hoist them into `AssignTemp` before per-element iteration, and replace with `TempArray` references
- `src/simlin-engine/src/compiler/mod.rs` -- `extract_temp_sizes()` must account for the new temps

**Dependencies:** None (independent of Phase 1)

**Done when:** Interpreter produces correct per-element values for VectorSortOrder, VectorElmMap, and AllocateAvailable in A2A contexts. Existing tests pass. Covers vm-vector-ops.AC4.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Bytecode opcodes and VM helpers

**Goal:** Define the four new opcodes and the `read_view_element` VM helper.

**Components:**
- `src/simlin-engine/src/bytecode.rs` -- add `VectorSelect`, `VectorElmMap`, `VectorSortOrder`, `AllocateAvailable` to the `Opcode` enum
- `src/simlin-engine/src/vm.rs` -- add `read_view_element` helper function for extracting a single element from a `RuntimeView` at a given flat index

**Dependencies:** None (independent of Phases 1-2)

**Done when:** Code compiles. Opcodes are defined but not yet emitted or dispatched.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Codegen emission

**Goal:** The compiler emits the new opcodes for all four builtins.

**Components:**
- `src/simlin-engine/src/compiler/codegen.rs` -- replace the `TodoArrayBuiltin` error block with opcode emission:
  - VECTOR SELECT: inline in `walk_expr` (push views, push scalars, emit `VectorSelect`, pop views)
  - VectorElmMap / VectorSortOrder / AllocateAvailable: in the `AssignTemp` handler, detect these builtins as RHS and emit the dedicated opcode instead of `BeginIter` loop

**Dependencies:** Phase 2 (expressions are now wrapped in AssignTemp), Phase 3 (opcodes exist)

**Done when:** Models with these builtins compile to bytecode without error. Covers vm-vector-ops.AC1.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: VM dispatch

**Goal:** The VM executes all four new opcodes, producing correct results.

**Components:**
- `src/simlin-engine/src/vm.rs` -- dispatch handlers for each opcode:
  - `VectorSelect`: iterate both views with selection filter, reduce with action
  - `VectorElmMap`: iterate offset view, indirect-index into source view, write to temp
  - `VectorSortOrder`: collect elements into SmallVec, sort, write 1-based indices to temp
  - `AllocateAvailable`: collect requests and profiles into SmallVecs, call shared `allocate_available`, write to temp

**Dependencies:** Phase 1 (shared alloc helpers), Phase 3 (opcodes defined), Phase 4 (opcodes emitted)

**Done when:** VM simulation of vector_simple and allocate models produces correct results matching interpreter output. Covers vm-vector-ops.AC1, AC2, AC3.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Test upgrades and validation

**Goal:** All three interpreter-only tests run through the full VM+interpreter path and pass.

**Components:**
- `src/simlin-engine/tests/simulate.rs` -- change `simulates_vector_simple_mdl` from `simulate_mdl_path_interpreter_only` to `simulate_mdl_path`
- `src/simlin-engine/tests/simulate.rs` -- change `simulates_allocate_mdl` from `simulate_mdl_path_interpreter_only` to `simulate_mdl_path`
- `src/simlin-engine/tests/simulate.rs` -- change `simulates_allocate_xmile` from `simulate_path_interpreter_only` to `simulate_path`
- `test/sdeverywhere/models/vector_simple/` -- add expected output `.dat` file with known-correct values for VectorElmMap and VectorSortOrder results

**Dependencies:** Phase 5 (VM dispatch works)

**Done when:** All three tests pass with VM+interpreter comparison. Expected output values validated against hand-computation or Vensim reference. Covers vm-vector-ops.AC3.
<!-- END_PHASE_6 -->

## Additional Considerations

**VECTOR SELECT action parameter:** The action is a runtime expression (can be `Time+6`, not just a constant). The opcode evaluates it from the arithmetic stack at runtime, matching the interpreter's `eval(action_expr).round() as i32` pattern. The error handling parameter (5th argument) is ignored, matching the interpreter.

**Sort stability:** VectorSortOrder uses Rust's `sort_by` (stable sort), matching the interpreter. This matters when elements have equal values -- the original order is preserved.

**SmallVec sizing:** Inline capacity of 32 for value buffers covers the vast majority of SD model dimensions (typically 3-20 elements). Arrays larger than 32 elements will heap-allocate the SmallVec, which is correct but slightly slower. This is an acceptable trade-off.
