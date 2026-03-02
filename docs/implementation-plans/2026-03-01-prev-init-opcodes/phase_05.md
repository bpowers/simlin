# Activate PREVIOUS/INIT Opcodes -- Phase 5

**Goal:** Final verification: run the full test suite (including pre-commit hooks) and confirm all acceptance criteria are satisfied.

**Architecture:** No code changes. This phase runs the full CI-equivalent verification and documents the completed state.

**Tech Stack:** Rust (simlin-engine), pre-commit hooks

**Scope:** Cross-cutting verification

**Codebase verified:** 2026-03-01

---

## Acceptance Criteria Coverage

This phase verifies ALL acceptance criteria are met:

### prev-init-opcodes.AC1: 1-arg PREVIOUS compiles to LoadPrev
- **prev-init-opcodes.AC1.1 Success:** `PREVIOUS(x)` in a scalar user model equation emits `LoadPrev` opcode (not module expansion)
- **prev-init-opcodes.AC1.2 Success:** `PREVIOUS(x[DimA])` in an arrayed equation emits per-element `LoadPrev` with correct offsets
- **prev-init-opcodes.AC1.3 Failure:** `PREVIOUS(x)` at the first timestep returns 0 (not x's initial value), matching module behavior

### prev-init-opcodes.AC2: INIT compiles to LoadInitial
- **prev-init-opcodes.AC2.1 Success:** `INIT(x)` in a user model equation emits `LoadInitial` opcode
- **prev-init-opcodes.AC2.2 Success:** `INIT(x)` in an aux-only model (no stocks) returns x's initial value correctly (Initials runlist includes x)
- **prev-init-opcodes.AC2.3 Failure:** `initial_values` snapshot is not all zeros when INIT references a variable that has no stock dependency

### prev-init-opcodes.AC3: 2-arg PREVIOUS preserved
- **prev-init-opcodes.AC3.1 Success:** `PREVIOUS(x, init_val)` still uses module expansion and produces identical results to pre-change behavior
- **prev-init-opcodes.AC3.2 Success:** Arrayed 2-arg PREVIOUS creates per-element module instances as before

### prev-init-opcodes.AC4: LTM uses direct opcodes
- **prev-init-opcodes.AC4.1 Success:** LTM link-score equations with `PREVIOUS(y)` compile to `LoadPrev` and produce correct loop scores matching reference data
- **prev-init-opcodes.AC4.2 Success:** PREVIOUS-specific module handling removed from `compile_ltm_equation_fragment()` without affecting SMOOTH/DELAY module support

### prev-init-opcodes.AC5: INIT stdlib model deleted
- **prev-init-opcodes.AC5.1 Success:** `stdlib/init.stmx` deleted, "init" removed from `MODEL_NAMES` and `stdlib_args`, no compilation errors

### prev-init-opcodes.AC6: Cross-cutting
- **prev-init-opcodes.AC6.1 Success:** All existing integration tests pass (previous, builtin_init, LTM test models)
- **prev-init-opcodes.AC6.2 Success:** Scaffolding comments in bytecode.rs, codegen.rs, builtins_visitor.rs updated to reflect activated state

---

<!-- START_TASK_1 -->
### Task 1: Run full test suite

**Verifies:** prev-init-opcodes.AC6.1

**Files:** None (verification only)

**Verification:**

Run: `cargo test -p simlin-engine`
Expected: All tests pass, including:

Integration tests (simulate.rs):
- `simulates_previous` -- 2-arg PREVIOUS via module expansion
- `simulates_init_builtin` -- 1-arg INIT via LoadInitial opcode
- All `TEST_MODELS` and `TEST_SDEVERYWHERE_MODELS`

LTM tests (simulate_ltm.rs):
- `simulates_population_ltm` -- PREVIOUS in LTM via LoadPrev
- `test_smooth_with_initial_value_ltm` -- SMOOTH module support preserved
- All discovery tests

Unit tests (db_tests.rs):
- `test_previous_opcode_interpreter_vm_parity`
- `test_init_opcode_interpreter_vm_parity`
- `test_previous_returns_zero_at_first_timestep`
- `test_2arg_previous_uses_module_expansion`
- `test_init_aux_only_model`
- `test_previous_still_in_stdlib_model_names`
- `test_previous_of_flow_interpreter_vm_parity`
- `test_previous_self_initial_value`

Also verify:
- `cargo clippy -p simlin-engine` passes
- `cargo fmt -p simlin-engine -- --check` passes
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Delete design plan document

**Files:**
- Delete: `docs/design-plans/2026-03-01-prev-init-opcodes.md`

**Implementation:**

The design plan has been fully implemented. Remove it from the repository.

```bash
git rm docs/design-plans/2026-03-01-prev-init-opcodes.md
```

**Commit:**
```bash
git add docs/design-plans/2026-03-01-prev-init-opcodes.md
git commit -m "docs: remove prev-init-opcodes design plan"
```
<!-- END_TASK_2 -->
