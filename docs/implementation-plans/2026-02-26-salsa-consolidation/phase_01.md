# Salsa Consolidation Phase 1: PREVIOUS and INIT as Builtin Opcodes

**Goal:** Replace the stdlib dynamic module implementations of PREVIOUS and INIT with VM intrinsic opcodes, eliminating per-call module instantiation overhead.

**Architecture:** PREVIOUS(x) and INIT(x) currently expand into synthetic sub-models with internal stock variables (5 variables for PREVIOUS, 2 for INIT). This phase promotes them to first-class builtins: new `BuiltinFn` variants, new `Opcode`/`SymbolicOpcode` variants, compiler emission, and VM dispatch. The stdlib module definitions are then removed.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** Phase 1 of 6 from original design

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### salsa-consolidation.AC6: PREVIOUS and INIT as builtins
- **salsa-consolidation.AC6.1 Success:** `PREVIOUS(x)` compiles to a single `LoadPrev` opcode instead of a 5-variable module instantiation.
- **salsa-consolidation.AC6.2 Success:** `INIT(x)` compiles to a single `LoadInitial` opcode.
- **salsa-consolidation.AC6.3 Success:** Nested `PREVIOUS(PREVIOUS(x))` in LTM equations compiles to two sequential `LoadPrev` opcodes (no module expansion).
- **salsa-consolidation.AC6.4 Success:** `previous` and `init` are no longer in `stdlib::MODEL_NAMES`.

### salsa-consolidation.AC5: All existing tests pass
- **salsa-consolidation.AC5.4 Success:** PREVIOUS and INIT behave identically as builtins -- no numerical differences from the stdlib module implementations.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Add BuiltinFn::Previous and BuiltinFn::Init variants

**Verifies:** None (type-level scaffolding; verified by compilation)

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs` (BuiltinFn enum at line 57, is_builtin_fn at line 288)

**Implementation:**

Add two new variants to the `BuiltinFn<Expr>` enum:
- `Previous(Box<Expr>)` -- takes the variable-reference expression whose previous-timestep value to load. The initial value argument is handled by the visitor (see Task 3) and not stored in the builtin variant because `LoadPrev` reads from `curr[]` where the initial value is already the variable's t=0 value.
- `Init(Box<Expr>)` -- takes the variable-reference expression whose t=0 value to freeze.

Update `is_builtin_fn(name: &str) -> bool` to return `true` for `"previous"` and `"init"`.

Update all exhaustive `match` arms on `BuiltinFn` throughout the file:
- `name()` method: return `"previous"` / `"init"`
- `try_map()`: recursively map the inner `Box<Expr>` (same pattern as other single-arg builtins like `Abs`)
- `for_each_expr_ref()`: visit the inner expression

Also update the type alias in `src/simlin-engine/src/compiler/expr.rs` (line 31) if needed -- it should already work since `BuiltinFn` is generic.

**Verification:**
Run: `cargo build -p simlin-engine`
Expected: Compiles with no errors. Existing tests still pass (nothing emits the new variants yet).

**Commit:** `engine: add BuiltinFn::Previous and BuiltinFn::Init variants`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add LoadPrev and LoadInitial opcodes and symbolic counterparts

**Verifies:** None (type-level scaffolding; verified by compilation)

**Files:**
- Modify: `src/simlin-engine/src/bytecode.rs` (search for `enum Opcode` and `fn stack_effect`)
- Modify: `src/simlin-engine/src/compiler/symbolic.rs` (search for `enum SymbolicOpcode`, `fn symbolize`, `fn resolve`)

**Implementation:**

In `bytecode.rs`, add two new `Opcode` variants:
- `LoadPrev { off: VariableOffset }` -- pushes `curr[module_off + off]` onto the stack. Semantically distinct from `LoadVar` (signals previous-timestep access for dependency tracking), but identical VM behavior.
- `LoadInitial { off: VariableOffset }` -- pushes the value from the initial-value buffer at `off` onto the stack.

Both have `stack_effect() -> i32 = 1` (push one value onto the operand stack, same as `LoadVar`).

In `compiler/symbolic.rs`, add two new `SymbolicOpcode` variants:
- `SymLoadPrev { var: SymVarRef }` -- symbolic counterpart using variable name instead of raw offset.
- `SymLoadInitial { var: SymVarRef }` -- symbolic counterpart.

Update the `symbolize()` function to map `Opcode::LoadPrev { off }` to `SymbolicOpcode::SymLoadPrev { var }` (same pattern as `LoadVar` -> `SymLoadVar`).

Update the `resolve()` function to map `SymbolicOpcode::SymLoadPrev { var }` back to `Opcode::LoadPrev { off }` by looking up the variable offset in the layout (same pattern as `SymLoadVar` -> `LoadVar`).

Same for `SymLoadInitial` <-> `LoadInitial`.

**Verification:**
Run: `cargo build -p simlin-engine`
Expected: Compiles with no errors. Existing tests still pass (nothing emits the new opcodes yet).

**Commit:** `engine: add LoadPrev and LoadInitial opcodes with symbolic counterparts`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-5) -->

<!-- START_TASK_3 -->
### Task 3: Wire BuiltinVisitor to emit Previous/Init builtins

**Verifies:** salsa-consolidation.AC6.1, salsa-consolidation.AC6.2, salsa-consolidation.AC6.3, salsa-consolidation.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs` (walk() method at line 285, around lines 297-392)

**Implementation:**

In the `walk()` method, the current path for PREVIOUS/INIT is:
1. Parser produces `App(UntypedBuiltinFn("previous", args), loc)`
2. `is_builtin_fn("previous")` returns false (currently)
3. `MODEL_NAMES.contains("previous")` returns true
4. Synthetic module expansion creates a Module variable + output Var

After Task 1, `is_builtin_fn("previous")` returns `true`. This means the existing code path at line ~305 intercepts it before module expansion:
```rust
if is_builtin_fn(&func) {
    return Ok(App(UntypedBuiltinFn(func, args), loc));
}
```

But this returns an `UntypedBuiltinFn`, not a typed `BuiltinFn`. The typed builtin resolution happens in `resolve_ident_and_lower()` (the expression lowering phase), which converts `UntypedBuiltinFn` names to concrete `BuiltinFn` variants.

Find where `UntypedBuiltinFn("abs", ...)` etc. are resolved to `BuiltinFn::Abs(...)` and add cases for:
- `"previous"` with 1 arg -> `BuiltinFn::Previous(arg)` (default initial value = variable's own t=0 value via `curr[]`)
- `"init"` with 1 arg -> `BuiltinFn::Init(arg)`

**Fallback strategy for PREVIOUS(x, iv) with custom initial value:**
The 2-arg form `PREVIOUS(x, iv)` has different t=0 semantics than `LoadPrev`:
- Current stdlib module: at t=0, returns `iv` (default 0)
- `LoadPrev { off }`: at t=0, returns `curr[off]` = x's own initial value

**Resolution:** Only the 1-arg form `PREVIOUS(x)` maps to `BuiltinFn::Previous(arg)` -> `LoadPrev`. When the 2-arg form `PREVIOUS(x, iv)` is encountered:
- If `iv` is the same variable as `x` (i.e., `PREVIOUS(x, x)`), emit `BuiltinFn::Previous(arg)` since the initial value matches.
- Otherwise, retain the module expansion path for that call site. Do NOT intercept it as a builtin -- let it fall through to `MODEL_NAMES.contains("previous")`. This means "previous" must remain in `MODEL_NAMES` until all 2-arg callers are eliminated (which may happen naturally in Phase 2 when LTM equations are compiled directly).

In practice, LTM equations use `PREVIOUS(x)` (1-arg form). No test model uses the 2-arg form with a custom initial value. Verify this during implementation by searching for `previous(` in test model files and LTM equation generation code.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All tests pass with identical numerical results.

**Commit:** `engine: wire BuiltinVisitor to emit Previous/Init builtins`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Update compiler codegen to emit SymLoadPrev/SymLoadInitial

**Verifies:** salsa-consolidation.AC6.1, salsa-consolidation.AC6.2, salsa-consolidation.AC6.3

**Files:**
- Modify: `src/simlin-engine/src/compiler/codegen.rs` (walk_expr method, around the BuiltinFn dispatch section starting at line 476)

**Implementation:**

In the compiler's `walk_expr()` method, add cases for `BuiltinFn::Previous` and `BuiltinFn::Init` in the `Expr::App` match arm.

For `BuiltinFn::Previous(arg)`:
1. Evaluate `arg` -- it should be an `Expr::Var(name)`. Extract the variable name.
2. Emit `SymbolicOpcode::SymLoadPrev { var: SymVarRef { name, element_offset: 0 } }`
3. If `arg` is not a simple `Var`, report an error (PREVIOUS requires a variable reference, not an arbitrary expression).

For `BuiltinFn::Init(arg)`:
1. Same pattern: extract variable name from `arg`
2. Emit `SymbolicOpcode::SymLoadInitial { var: SymVarRef { name, element_offset: 0 } }`
3. Error if not a simple `Var`.

For `PREVIOUS(PREVIOUS(x))` (AC6.3): The inner `PREVIOUS(x)` is itself a `BuiltinFn::Previous(Var("x"))`. The outer PREVIOUS wraps this. However, `PREVIOUS(PREVIOUS(x))` means "the previous value of the previous value of x" which requires that the inner PREVIOUS result is stored as a variable. This needs careful handling:
- Option A: Nested PREVIOUS(PREVIOUS(x)) compiles to two sequential `SymLoadPrev` opcodes for x (each reads the same `curr[off]`). This is what the design says but gives identical values, which may not be the intended LTM semantics.
- Option B: The inner PREVIOUS gets its own synthetic variable slot so the outer PREVIOUS can reference it.

Check how LTM equations generate nested PREVIOUS expressions and verify the expected behavior. The design says "compiles to two sequential LoadPrev opcodes" so follow that unless tests disagree.

**Note:** This task MUST be committed together with Tasks 3 and 5 (or immediately after Task 3 and before running tests that exercise PREVIOUS/INIT). The visitor, codegen, and VM changes form an atomic unit -- the visitor produces new AST nodes, codegen emits new opcodes, and the VM must handle them.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All tests pass. PREVIOUS/INIT now compile through the new opcode path.

**Commit:** `engine: emit SymLoadPrev/SymLoadInitial in codegen for Previous/Init builtins`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Implement VM dispatch for LoadPrev and LoadInitial

**Verifies:** salsa-consolidation.AC6.1, salsa-consolidation.AC6.2, salsa-consolidation.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/vm.rs` (eval_bytecode at line 966)

**Implementation:**

Add match arms in the VM's `eval_bytecode()` dispatch loop for:

`Opcode::LoadPrev { off }`:
```rust
Opcode::LoadPrev { off } => {
    let value = curr[module_off + *off as usize];
    stack.push(value);
}
```
This is semantically identical to `LoadVar` -- during the dt-phase evaluation, `curr[]` holds the previous timestep's committed values. The variable's value in `curr[off]` IS its previous-timestep value.

`Opcode::LoadInitial { off }`:
The VM must read from the initial-value buffer captured at t=0. The design says "The VM already stores initial values." Verify the VM struct has an initial values field. Likely stored as a separate buffer populated during the initial evaluation phase. Implementation:
```rust
Opcode::LoadInitial { off } => {
    let value = initial_values[module_off + *off as usize];
    stack.push(value);
}
```
If the VM does NOT currently store initial values separately, add an `initial_values: Vec<f64>` field to the VM struct, populated by copying `curr[]` after the initial evaluation phase (t=0). This is a one-time snapshot.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All simulation tests pass with identical numerical results to the pre-change baseline. This is the critical verification for AC5.4.

**Commit:** `engine: implement VM dispatch for LoadPrev and LoadInitial opcodes`
<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 6-7) -->

<!-- START_TASK_6 -->
### Task 6: Remove "previous" and "init" from stdlib::MODEL_NAMES

**Verifies:** salsa-consolidation.AC6.4

**Files:**
- Modify: `src/simlin-engine/src/stdlib.gen.rs` (MODEL_NAMES at line 20, `get()` function at line 24, model definitions for init at lines 431-488 and previous at lines 490-628)

**Implementation:**

1. Remove `"init"` and `"previous"` from the `MODEL_NAMES` array. Change from `[&str; 7]` to `[&str; 5]` containing only: `"delay1", "delay3", "smth1", "smth3", "trend"`.

2. Remove the corresponding match arms in the `get(name: &str) -> Option<Model>` function for `"init"` and `"previous"`.

3. The model definition code for init (lines 431-488) and previous (lines 490-628) can be deleted entirely since no code path creates these models anymore.

**Note:** After this change, if someone writes `previous(x)` in a model equation:
- `is_builtin_fn("previous")` returns true (from Task 1)
- The visitor creates `BuiltinFn::Previous(x)` (from Task 3)
- The MODEL_NAMES path is never reached
- Compilation works correctly through the new opcode path

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All tests pass. No code path references the deleted stdlib models.

**Commit:** `engine: remove previous and init from stdlib MODEL_NAMES`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Add verification tests for PREVIOUS/INIT as builtins

**Verifies:** salsa-consolidation.AC6.1, salsa-consolidation.AC6.2, salsa-consolidation.AC6.3, salsa-consolidation.AC6.4, salsa-consolidation.AC5.4

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs` (add new test functions)
- Test utility: `src/simlin-engine/src/test_common.rs` (TestProject builder)

**Testing:**

Tests must verify each AC listed above:
- **salsa-consolidation.AC6.1:** Build a `TestProject` with an aux using `PREVIOUS(some_stock)`. Compile and inspect the generated bytecode -- verify it contains `LoadPrev` and does NOT contain `EvalModule` for a previous-module. Compare interpreter vs VM results to confirm numerical equivalence.
- **salsa-consolidation.AC6.2:** Build a `TestProject` with an aux using `INIT(some_stock)`. Compile and inspect bytecode -- verify `LoadInitial` opcode is present, no module expansion. Verify the INIT value stays constant across all timesteps.
- **salsa-consolidation.AC6.3:** Build a `TestProject` with an LTM-style equation using `PREVIOUS(PREVIOUS(x))`. Compile and verify two `LoadPrev` opcodes are emitted (no module expansion). This test may need to construct the equation string manually.
- **salsa-consolidation.AC6.4:** Assert `!stdlib::MODEL_NAMES.contains(&"previous")` and `!stdlib::MODEL_NAMES.contains(&"init")`.
- **salsa-consolidation.AC5.4:** The existing tests in `tests/simulate.rs` and `tests/simulate_ltm.rs` already cross-validate interpreter vs VM. Running the full test suite (done in verification of previous tasks) covers this. Add an explicit test that constructs a model with PREVIOUS and verifies VM output matches interpreter output value-by-value.

Follow the `TestProject` builder pattern from `test_common.rs`:
```rust
TestProject::new("test_previous_builtin")
    .with_sim_time(0.0, 10.0, 1.0)
    .stock("level", "100", &["inflow"], &[], None)
    .flow("inflow", "10", None)
    .aux("prev_level", "PREVIOUS(level)", None)
    .compile()
    .expect("should compile");
```

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All new and existing tests pass.

**Commit:** `engine: add verification tests for PREVIOUS/INIT as builtins`
<!-- END_TASK_7 -->

<!-- END_SUBCOMPONENT_C -->
