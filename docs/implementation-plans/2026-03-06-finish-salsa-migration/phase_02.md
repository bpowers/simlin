# Finish Salsa Migration Implementation Plan

**Goal:** Make the incremental salsa compilation pipeline the sole compilation path, then delete the monolithic code.

**Architecture:** The salsa-based incremental pipeline (`compile_project_incremental` / `assemble_simulation`) is already the production default. This plan migrates remaining callers, tests, and infrastructure, then deletes the monolithic path (`Project::from`, `Simulation::compile`, `compile_project`).

**Tech Stack:** Rust, salsa (incremental computation framework)

**Scope:** 7 phases from original design (phases 1-7)

**Codebase verified:** 2026-03-06

---

## Acceptance Criteria Coverage

This phase implements and tests:

### finish-salsa-migration.AC2: Incremental path never panics on malformed models
- **finish-salsa-migration.AC2.1 Success:** Compiling a model with unknown builtins (e.g., Vensim macros) through `compile_project_incremental` returns `Err(NotSimulatable)`, not a panic.
- **finish-salsa-migration.AC2.2 Success:** Compiling a model with missing module references returns `Err`, not a panic.
- **finish-salsa-migration.AC2.3 Success:** `catch_unwind` wrappers removed from benchmarks (`benches/compiler.rs`), tests (`tests/simulate.rs`), and incremental layout paths (`layout/mod.rs`).
- **finish-salsa-migration.AC2.4 Success:** `compile_project_incremental` docstring accurately describes current behavior (no stale monolithic-fallback claim).

---

## Phase 2: Remove Vestigial `catch_unwind` Wrappers

**Phase type:** Infrastructure -- removing defensive wrappers and updating documentation. No new functionality.

**Verifies:** None (existing tests already validate that the incremental path returns clean errors)

**Note on line numbers:** Line numbers referenced below are approximate and may shift as earlier phases modify files. Use function name search rather than relying on exact line numbers.

**Note:** Three `catch_unwind` sites wrapping monolithic code are deferred to Phase 4:
- `src/simlin-engine/src/layout/mod.rs:1874` (`try_compile_model`, monolithic `Project::from`)
- `src/simlin-engine/src/layout/mod.rs:2129` (`try_detect_ltm_loops_monolithic`, monolithic LTM pipeline)
- `src/simlin-engine/src/analysis.rs:124` (`run_ltm_pipeline`, LTM pipeline)

<!-- START_TASK_1 -->
### Task 1: Remove catch_unwind from benches/compiler.rs

**Files:**
- Modify: `src/simlin-engine/benches/compiler.rs:71-95` (the `is_simulatable` function)

**Step 1: Refactor `is_simulatable` to use Result propagation**

The function at line 73 currently wraps the incremental compile pipeline in `catch_unwind`. Replace it so the function returns `Result<bool, ...>` (or simply `bool` with `Result::is_ok()` matching) instead of catching panics.

The current pattern is approximately:
```rust
fn is_simulatable(project: &datamodel::Project) -> bool {
    // Uses catch_unwind because ...
    std::panic::catch_unwind(|| {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, project, None);
        compile_project_incremental(&db, sync.project, "main")
    })
    .is_ok()
}
```

Replace with:
```rust
fn is_simulatable(project: &datamodel::Project) -> bool {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    compile_project_incremental(&db, sync.project, "main").is_ok()
}
```

Remove the `use std::panic` import if it becomes unused.

**Step 2: Verify benchmarks compile**

```bash
cargo check -p simlin-engine --benches
```

Expected: Compiles without errors.
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Remove catch_unwind from tests/simulate.rs

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs:1294` (inside `incremental_compilation_covers_all_models`)

**Step 1: Replace catch_unwind with Result matching**

The test at line ~1294 uses `catch_unwind` to wrap the incremental compile pipeline, treating panics as test failures for specific models. Replace with direct `Result` matching since `compile_project_incremental` now returns clean `Err` values.

The current pattern wraps a closure in `catch_unwind` and checks `is_ok()`. Replace so the test calls `compile_project_incremental` directly and matches on `Result::Ok` / `Result::Err` without catching panics.

Remove the `use std::panic` import if it becomes unused in this file.

**Step 2: Verify the test compiles and passes**

```bash
cargo test -p simlin-engine --test simulate incremental_compilation_covers_all_models -- --nocapture
```

Expected: Test passes.
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Remove catch_unwind from layout/mod.rs incremental paths

**Files:**
- Modify: `src/simlin-engine/src/layout/mod.rs:2038-2045` (catch_unwind around `model_detected_loops`)
- Modify: `src/simlin-engine/src/layout/mod.rs:2058-2075` (catch_unwind around incremental compile + VM)

**Important:** Only modify the two `catch_unwind` sites inside `try_detect_ltm_loops_incremental`. Do NOT touch:
- Line 1874 (`try_compile_model`, monolithic -- deferred to Phase 4)
- Line 2129 (`try_detect_ltm_loops_monolithic` -- deferred to Phase 4)

**Step 1: Remove catch_unwind around model_detected_loops (line ~2041)**

The current code wraps `model_detected_loops(db, ...)` in `catch_unwind`. Since this is a salsa tracked function that returns `Result`, replace with direct invocation. If the function can return an error, propagate it or handle it with `?` / `.ok()` matching.

**Step 2: Remove catch_unwind around incremental compile + VM (line ~2061)**

The current code wraps `compile_project_incremental` + `Vm::new` + `vm.run_to_end` in `catch_unwind`. Replace with direct `Result` propagation -- `compile_project_incremental` returns `Result`, and the VM execution chain can be handled with `?` or `.ok()`.

**Step 3: Verify compilation**

```bash
cargo check -p simlin-engine
```

Expected: Compiles without errors.

**Step 4: Run layout-related tests**

```bash
cargo test -p simlin-engine --test layout
cargo test -p simlin-engine layout
```

Expected: All layout tests pass.
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Update stale docstring on compile_project_incremental

**Files:**
- Modify: `src/simlin-engine/src/db.rs:5796-5800` (docstring on `compile_project_incremental`)

**Step 1: Replace stale docstring**

The current docstring at lines 5796-5800 reads:
```rust
/// Compile a project incrementally using salsa.
///
/// This is the new entry point that replaces compile_project for the
/// incremental path. Falls back to the monolithic compile_project when
/// the incremental path is not yet supported (e.g., multi-model projects).
```

The "Falls back to the monolithic compile_project" claim is stale -- the function body contains no fallback; it calls `assemble_simulation` directly and maps errors to `NotSimulatable`. Replace with an accurate docstring:

```rust
/// Compile a project incrementally using salsa tracked functions.
///
/// This is the production compilation entry point. Returns the assembled
/// `CompiledSimulation` for the named model, or `Err(NotSimulatable)` if
/// compilation fails (e.g., unresolved references, unsupported builtins).
```

**Step 2: Verify compilation**

```bash
cargo check -p simlin-engine
```

Expected: Compiles without errors.
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Commit closing #363

**Step 1: Run full test suite to confirm nothing broke**

```bash
cargo test -p simlin-engine
```

Expected: All tests pass.

**Step 2: Stage and commit**

```bash
git add src/simlin-engine/benches/compiler.rs src/simlin-engine/tests/simulate.rs src/simlin-engine/src/layout/mod.rs src/simlin-engine/src/db.rs
git commit -m "engine: remove catch_unwind from incremental compilation paths

The incremental compiler returns clean Result::Err for all error conditions
instead of panicking, so defensive catch_unwind wrappers are no longer
needed. Removes catch_unwind from benchmarks, the incremental coverage
test, and the incremental LTM loop detection path.

Also updates the compile_project_incremental docstring which incorrectly
claimed a monolithic fallback that no longer exists.

Three catch_unwind sites wrapping monolithic code (layout/mod.rs:1874,
layout/mod.rs:2129, analysis.rs:124) are retained until those callers
migrate to the incremental path in Phase 4.

Fix #363"
```
<!-- END_TASK_5 -->
