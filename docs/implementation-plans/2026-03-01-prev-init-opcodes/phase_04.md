# Activate PREVIOUS/INIT Opcodes -- Phase 4

**Goal:** Add new test coverage for activated opcode paths and update documentation comments.

**Architecture:** Add tests verifying LoadPrev/LoadInitial opcode emission, arrayed PREVIOUS/INIT, INIT in aux-only models, and first-timestep PREVIOUS=0 behavior. Update scaffolding comments in builtins_visitor.rs. Rename existing parity tests to reflect they now exercise the opcode path.

**Tech Stack:** Rust (simlin-engine)

**Scope:** Design Phase 5

**Codebase verified:** 2026-03-01

---

## Acceptance Criteria Coverage

This phase implements and tests:

### prev-init-opcodes.AC1: 1-arg PREVIOUS compiles to LoadPrev
- **prev-init-opcodes.AC1.3 Failure:** `PREVIOUS(x)` at the first timestep returns 0 (not x's initial value), matching module behavior

### prev-init-opcodes.AC3: 2-arg PREVIOUS preserved
- **prev-init-opcodes.AC3.1 Success:** `PREVIOUS(x, init_val)` still uses module expansion and produces identical results to pre-change behavior

### prev-init-opcodes.AC6: Cross-cutting
- **prev-init-opcodes.AC6.1 Success:** All existing integration tests pass (previous, builtin_init, LTM test models)
- **prev-init-opcodes.AC6.2 Success:** Scaffolding comments in bytecode.rs, codegen.rs, builtins_visitor.rs updated to reflect activated state

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Update existing parity tests for opcode path

**Verifies:** prev-init-opcodes.AC6.1

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs:4838-4942` (existing parity tests)

**Implementation:**

The existing tests `test_previous_module_expansion_interpreter_vm_parity` (line 4839) and `test_init_module_expansion_interpreter_vm_parity` (line 4903) now exercise the opcode path (not module expansion) for 1-arg PREVIOUS and INIT. Update their names and docstrings to reflect this.

**Rename `test_previous_module_expansion_interpreter_vm_parity`:**

Change the function name and update the docstring to note that 1-arg PREVIOUS now compiles to LoadPrev:

```rust
/// 1-arg PREVIOUS(x) compiles to the LoadPrev opcode. Verify that
/// interpreter and VM produce identical results, and that PREVIOUS
/// returns 0 at the first timestep (matching the old module behavior
/// where initial_value defaults to 0).
#[test]
fn test_previous_opcode_interpreter_vm_parity() {
```

**Rename `test_init_module_expansion_interpreter_vm_parity`:**

```rust
/// INIT(x) compiles to the LoadInitial opcode. Verify that interpreter
/// and VM produce identical results, and that INIT freezes the t=0
/// value correctly even in an aux-only model (no stocks).
#[test]
fn test_init_opcode_interpreter_vm_parity() {
```

The test bodies don't need changes -- they already verify the correct behavior.

**Commit:** Do not commit yet (combine with remaining tasks)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add new tests for opcode paths

**Verifies:** prev-init-opcodes.AC1.3, prev-init-opcodes.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs` (add new tests)

**Implementation:**

Add the following tests to `src/simlin-engine/src/db_tests.rs`:

**Test 1: PREVIOUS returns 0 at first timestep (AC1.3)**

```rust
/// prev-init-opcodes.AC1.3: PREVIOUS(x) at the first timestep returns 0
/// (not x's initial value). This is correct for the LoadPrev opcode path
/// because prev_values is initialized to zeros and not seeded with
/// post-initials state.
#[test]
fn test_previous_returns_zero_at_first_timestep() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("prev_zero_first_step")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("x", "42", None)
        .aux("prev_x", "PREVIOUS(x)", None);

    let vm = tp.run_vm().expect("VM should run");
    let prev_vals = vm.get("prev_x").expect("prev_x not in results");

    // At t=0, PREVIOUS(x) returns 0 (not 42)
    assert!(
        (prev_vals[0] - 0.0).abs() < 1e-10,
        "PREVIOUS at t=0 should be 0, got {}",
        prev_vals[0]
    );
    // At t=1+, PREVIOUS(x) returns 42 (x is constant)
    for step in 1..prev_vals.len() {
        assert!(
            (prev_vals[step] - 42.0).abs() < 1e-10,
            "PREVIOUS at step {step} should be 42, got {}",
            prev_vals[step]
        );
    }
}
```

**Test 2: 2-arg PREVIOUS still uses module expansion (AC3.1)**

```rust
/// prev-init-opcodes.AC3.1: 2-arg PREVIOUS(x, init_val) still uses module
/// expansion. Verify the init_val is returned at the first timestep
/// instead of 0 (confirming module path, not opcode path).
#[test]
fn test_2arg_previous_uses_module_expansion() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("prev_2arg")
        .with_sim_time(0.0, 3.0, 1.0)
        .stock("level", "100", &["inflow"], &[], None)
        .flow("inflow", "10", None)
        .aux("prev_level", "PREVIOUS(level, 99)", None);

    let vm = tp.run_vm().expect("VM should run");
    let prev_vals = vm.get("prev_level").expect("prev_level not in results");

    // At t=0, 2-arg PREVIOUS returns init_val=99 (not 0)
    assert!(
        (prev_vals[0] - 99.0).abs() < 1e-10,
        "2-arg PREVIOUS at t=0 should be 99, got {}",
        prev_vals[0]
    );
    // At t=1, returns level at t=0 = 100
    assert!(
        (prev_vals[1] - 100.0).abs() < 1e-10,
        "2-arg PREVIOUS at t=1 should be 100, got {}",
        prev_vals[1]
    );
}
```

**Test 3: INIT in aux-only model (AC2.2 reinforcement)**

```rust
/// Verify INIT works in a model with no stocks or modules, where the
/// Initials runlist extension is the only thing that ensures the
/// referenced variable is evaluated at t=0.
#[test]
fn test_init_aux_only_model() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("init_aux_only")
        .with_sim_time(1.0, 5.0, 1.0)
        .aux("growing", "TIME * 2", None)
        .aux("frozen", "INIT(growing)", None);

    let vm = tp.run_vm().expect("VM should run");
    let frozen_vals = vm.get("frozen").expect("frozen not in results");

    // INIT(growing) should freeze growing's t=0 value: TIME*2 = 1.0*2 = 2.0
    for (step, val) in frozen_vals.iter().enumerate() {
        assert!(
            (val - 2.0).abs() < 1e-10,
            "frozen should be 2.0 at every step, got {val} at step {step}"
        );
    }
}
```

Follow project testing patterns -- use `TestProject` builder from `src/simlin-engine/src/test_common.rs`.

**Verification:**

Run: `cargo test -p simlin-engine test_previous_returns_zero test_2arg_previous test_init_aux_only`
Expected: All new tests pass.

**Commit:** Do not commit yet (combine with Task 3)
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Update scaffolding comments

**Verifies:** prev-init-opcodes.AC6.2

**Files:**
- Modify: `src/simlin-engine/src/builtins_visitor.rs:328-345` (gate comment, already changed in Phase 1 -- verify)
- Modify: `src/simlin-engine/src/compiler/codegen.rs:931-932, 984` (stale "Task 4" comments)

**Implementation:**

**Step 1: Verify builtins_visitor.rs gate comment (Phase 1 already changed this)**

The gate comment at lines 328-345 was already replaced in Phase 1 with the new arity-based gate comment. Verify the comment accurately describes the activated state. If still referencing scaffolding/deferred activation, update to reflect that opcodes are now active.

**Step 2: Update stale codegen.rs comments**

In `src/simlin-engine/src/compiler/codegen.rs`, there are stale comments at lines 931-932 and 984 that reference "Task 4 of phase 1" and say "Until then, nothing emits these variants." The Previous/Init early-return path (lines 692-737) already exists and is now actively reached.

Update the comments at the `unreachable!` guards:

At line 931-932 (and similar at 984):
```rust
// Previous/Init are handled by the early-return path at the top
// of walk_builtin (LoadPrev/LoadInitial opcodes). Reaching here
// would be a logic error.
unreachable!("Previous/Init builtins should be handled before reaching BuiltinId dispatch");
```

Remove any references to "Task 4", "scaffolding", or "until then".

**Step 3: Update bytecode.rs opcode comments (if needed)**

The investigator found no scaffolding comments in `bytecode.rs:563-575` -- the LoadPrev and LoadInitial opcode docs are already clean and accurate. Verify and skip if nothing needs updating.

**Verification:**

Run: `cargo test -p simlin-engine`
Expected: All tests pass (no behavioral changes, only comments).

**Commit:**
```bash
git add src/simlin-engine/src/db_tests.rs src/simlin-engine/src/compiler/codegen.rs src/simlin-engine/src/builtins_visitor.rs
git commit -m "engine: add opcode-path tests and update comments for PREVIOUS/INIT"
```
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
