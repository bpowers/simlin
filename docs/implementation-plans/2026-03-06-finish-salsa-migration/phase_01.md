# Finish Salsa Migration Implementation Plan

**Goal:** Make the incremental salsa compilation pipeline the sole compilation path, then delete the monolithic code.

**Architecture:** The salsa-based incremental pipeline (`compile_project_incremental` / `assemble_simulation`) is already the production default. This plan migrates remaining callers, tests, and infrastructure, then deletes the monolithic path (`Project::from`, `Simulation::compile`, `compile_project`).

**Tech Stack:** Rust, salsa (incremental computation framework)

**Scope:** 7 phases from original design (phases 1-7)

**Codebase verified:** 2026-03-06

---

## Acceptance Criteria Coverage

This phase verifies (does not implement new code for):

### finish-salsa-migration.AC1: Incremental path handles all model types
- **finish-salsa-migration.AC1.1 Success:** Models with module variables compile through `compile_project_incremental` and produce identical simulation results to the monolithic path.
- **finish-salsa-migration.AC1.2 Success:** Models with SMOOTH/DELAY/TREND builtins compile through the incremental path with correct layout slots for implicit variables.
- **finish-salsa-migration.AC1.3 Success:** Multiple instances of the same sub-model with different input wirings produce distinct compiled module entries.
- **finish-salsa-migration.AC1.4 Success:** `test_incremental_compile_smooth_over_module_output` and `test_incremental_compile_distinguishes_module_input_sets` pass (existing coverage).

### finish-salsa-migration.AC6: Single sync path
- **finish-salsa-migration.AC6.1 Success:** No production caller invokes `sync_from_datamodel` directly; all go through `sync_from_datamodel_incremental`.
- **finish-salsa-migration.AC6.2 Success:** `sync_from_datamodel` remains as an internal bootstrap function called by `sync_from_datamodel_incremental` when `prev_state` is `None`.

---

## Phase 1: Verify and Close Already-Fixed Issues

**Phase type:** Infrastructure/verification -- no new functionality, only confirming existing code and updating stale documentation.

**Verifies:** None (verification phase -- existing tests already cover ACs listed above)

<!-- START_TASK_1 -->
### Task 1: Run existing incremental-path tests to confirm AC1 coverage

**Files:**
- Read: `src/simlin-engine/src/db_tests.rs` (tests at lines ~4004 and ~4100)

**Step 1: Run the two key tests that prove #295 is fixed**

```bash
cargo test -p simlin-engine test_incremental_compile_smooth_over_module_output test_incremental_compile_distinguishes_module_input_sets -- --nocapture
```

Expected: Both tests pass. These tests exercise:
- Module variables receiving stock-phase bytecodes (`compile_var_fragment` with `is_stock || is_module` guard)
- Implicit variables from SMOOTH getting layout slots (`compute_layout` calling `model_implicit_var_info`)
- `model_module_map` populating `module_models` in the compiler context
- `enumerate_module_instances` differentiating input sets per model instance

**Step 2: Run the full incremental compilation test suite**

```bash
cargo test -p simlin-engine db_tests -- --nocapture
```

Expected: All `db_tests` pass, confirming the incremental path handles all model types tested.

**Step 3: Run the integration test that exercises incremental compilation across all test models**

```bash
cargo test -p simlin-engine --test simulate incremental_compilation_covers_all_models -- --nocapture
```

Expected: Test passes (may use `catch_unwind` -- that's addressed in Phase 2).
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Verify sync_from_datamodel_incremental is the production sync path (#290)

**Files:**
- Read: `src/simlin-engine/src/db.rs` (function at line ~2645)
- Read: `src/libsimlin/src/patch.rs` (calls at lines ~454 and ~528)

**Step 1: Confirm sync_from_datamodel_incremental uses salsa setters**

Read `src/simlin-engine/src/db.rs` starting at the `sync_from_datamodel_incremental` function (line ~2645). Verify:
- It imports `use salsa::Setter;`
- It calls `.set_name(db).to(...)`, `.set_sim_specs(db).to(...)`, and per-variable setters
- When `prev_state` is `None`, it falls through to `sync_from_datamodel` (bootstrap path)

**Step 2: Confirm production caller uses incremental sync**

Read `src/libsimlin/src/patch.rs` at `apply_project_patch_internal`. Verify it calls `sync_from_datamodel_incremental` (not `sync_from_datamodel` directly) at lines ~454 and ~528.

**Step 3: No code changes needed**

This is a read-only verification step. The functionality is already correct.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Update stale docstrings referencing #295 in interpreter.rs

**Files:**
- Modify: `src/simlin-engine/src/interpreter.rs:1884` (Simulation::compile docstring)
- Modify: `src/simlin-engine/src/interpreter.rs:2100` (compile_project docstring)

**Step 1: Update Simulation::compile docstring**

At `src/simlin-engine/src/interpreter.rs:1884`, the docstring currently says something like "the incremental path does not yet correctly propagate module input values during VM simulation (GitHub #295). Once that is fixed, this method and `compile_project` can be removed."

Replace this with a docstring that reflects reality: `Simulation::compile` is the monolithic bytecode compilation path, retained only for test cross-validation. Production compilation uses `db::compile_project_incremental`. The module input propagation issue (#295) is fixed in the incremental path.

**Step 2: Update compile_project docstring**

At `src/simlin-engine/src/interpreter.rs:2100`, update the similarly stale docstring. The `compile_project` free function is the monolithic compilation entry point, retained only for test use. Production uses `compile_project_incremental`.

**Step 3: Verify the file compiles**

```bash
cargo check -p simlin-engine
```

Expected: Compiles without errors or warnings.
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Commit closing #290 and #295

**Step 1: Stage and commit**

```bash
git add src/simlin-engine/src/interpreter.rs
git commit -m "engine: update stale docstrings for monolithic compilation path

The incremental compilation path (compile_project_incremental) now correctly
handles all model types including module variables, implicit variables from
SMOOTH/DELAY/TREND, and differentiated module input sets. Update docstrings
on Simulation::compile and compile_project that incorrectly claimed #295
was still open.

Fix #290
Fix #295"
```

**Step 2: Verify tests still pass**

```bash
cargo test -p simlin-engine
```

Expected: All tests pass.
<!-- END_TASK_4 -->
