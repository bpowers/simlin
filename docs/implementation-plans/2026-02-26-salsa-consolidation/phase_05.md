# Salsa Consolidation Phase 5: Thread SimlinDb to Internal Callers

**Goal:** All internal engine code that needs simulation results uses the incremental path via `SimlinDb`. The monolithic `with_ltm()` / `with_ltm_all_links()` methods and all internal `Simulation::new().compile()` calls in production code are eliminated.

**Architecture:** Functions that build `engine::Project` internally just to compile and simulate (layout LTM scoring, analysis, CLI) gain a `db: &SimlinDb` parameter and use `compile_project_incremental` instead. The `with_ltm()` and `with_ltm_all_links()` methods on `engine::Project` are deleted since LTM is always-on in the salsa db (Phase 2). The CLI creates its own `SimlinDb` for standalone simulation.

**Tech Stack:** Rust (simlin-engine, libsimlin, simlin-cli, simlin-mcp crates)

**Scope:** Phase 5 of 6 from original design (depends on Phases 2 and 4)

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### salsa-consolidation.AC3: Old bytecode compilation path deleted (partial)
- **salsa-consolidation.AC3.3 Success:** `compile_simulation` in libsimlin no longer exists.

### salsa-consolidation.AC5: All existing tests pass
- **salsa-consolidation.AC5.1 Success:** All tests in `tests/simulate*.rs` pass with identical numerical results.
- **salsa-consolidation.AC5.2 Success:** All tests in `tests/simulate_ltm.rs` pass with identical numerical results.
- **salsa-consolidation.AC5.3 Success:** All libsimlin integration tests pass.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Thread db to layout LTM loop scoring

**Verifies:** salsa-consolidation.AC5.1

**Files:**
- Modify: `src/simlin-engine/src/layout/mod.rs` (try_detect_ltm_loops at line 2010)
- Modify: `src/simlin-engine/src/layout/mod.rs` (callers of try_detect_ltm_loops -- find the parent function that calls it)
- Modify: `src/libsimlin/src/layout.rs` (simlin_project_diagram_sync at line 67, which calls into engine layout)

**Implementation:**

`try_detect_ltm_loops` currently takes `&datamodel::Project` and internally builds an `engine::Project`, calls `with_ltm()`, then `Simulation::new().compile()` to get simulation results for LTM scoring.

Change the signature to accept a `&SimlinDb` (or `&dyn Db`) and a `SourceProject`:

```rust
fn try_detect_ltm_loops(
    db: &SimlinDb,
    project: SourceProject,
    model_name: &str,
) -> Option<LtmLoopResults> {
    // Set ltm_enabled on the project (may already be set)
    // Use compile_project_incremental to get compiled simulation
    // Use Vm::new(compiled) + vm.run_to_end() for results
    // Extract LTM scores from results
}
```

The db reference needs to be threaded from the caller. Trace the call chain:
1. `simlin_project_diagram_sync` (libsimlin FFI) calls `engine::layout::generate_best_layout`
2. `generate_best_layout` calls `try_detect_ltm_loops`

`generate_best_layout` needs a `db: &SimlinDb` parameter, which `simlin_project_diagram_sync` provides from `SimlinProject.db`.

With Phase 2's always-on LTM in the salsa pipeline, the db already contains LTM compilation results when `ltm_enabled=true`. The layout function just needs to set `ltm_enabled`, compile, and run.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io -- layout`
Expected: Layout tests pass with identical results.

**Commit:** `engine: thread SimlinDb to layout LTM loop scoring`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Thread db to analysis module

**Verifies:** salsa-consolidation.AC5.3

**Files:**
- Modify: `src/simlin-engine/src/analysis.rs` (analyze_model at line 65, run_ltm_pipeline at line 91)
- Modify: `src/simlin-mcp/src/tools/edit_model.rs` (line 225, caller of analyze_model)
- Modify: `src/simlin-mcp/src/tools/read_model.rs` (line 58, caller of analyze_model)
- Modify: `src/libsimlin/src/analysis.rs` (simlin_analyze_get_loops at line 45, simlin_analyze_get_links at line 213)

**Implementation:**

`analyze_model` currently takes `&datamodel::Project` and internally builds `engine::Project` + calls `with_ltm_all_links()` + `Simulation::new().compile()`.

Change signature to accept a `&SimlinDb` (or `&dyn Db`) and a `SourceProject`:

```rust
pub fn analyze_model(
    db: &SimlinDb,
    project: SourceProject,
    model_name: &str,
) -> Result<ModelAnalysis, String>
```

The `run_ltm_pipeline` helper changes similarly. Instead of building engine::Project and calling with_ltm_all_links(), it:
1. Sets `ltm_enabled` on the SourceProject (if not already set)
2. Calls `compile_project_incremental(db, project, model_name)`
3. Creates `Vm::new(compiled)` and runs `vm.run_to_end()`
4. Extracts LTM results from the VM output

Update callers:
- **MCP tools**: These need to create a `SimlinDb`, sync from the datamodel, then call `analyze_model`. The MCP tools currently have access to `&datamodel::Project`. Add db creation:
  ```rust
  let mut db = SimlinDb::default();
  let sync = sync_from_datamodel_incremental(&mut db, &project, None);
  let analysis = analyze_model(&db, sync.project, &model_name)?;
  ```

- **libsimlin analysis FFI**: `simlin_analyze_get_loops` and `simlin_analyze_get_links` currently use `engine::Project` from SimlinProject. After Phase 4, SimlinProject holds `datamodel::Project` + `db`. These functions should lock the db and use salsa tracked functions directly:
  - `simlin_analyze_get_loops`: Use `model_loop_circuits(db, model, project)` from the salsa pipeline
  - `simlin_analyze_get_links`: Use `model_causal_edges(db, model, project)` from the salsa pipeline

**Verification:**
Run: `cargo test -p simlin-engine --features file_io && cargo test -p libsimlin && cargo build -p simlin-mcp`
Expected: All analysis and integration tests pass. simlin-mcp compiles with the new analyze_model signature.

**Commit:** `engine: thread SimlinDb to analysis module`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Eliminate with_ltm() and with_ltm_all_links() from project.rs

**Verifies:** salsa-consolidation.AC5.2

**Files:**
- Modify: `src/simlin-engine/src/project.rs` (with_ltm at line 37, with_ltm_all_links at line 50)

**Implementation:**

After Tasks 1 and 2, all production callers of `with_ltm()` and `with_ltm_all_links()` have been migrated to the salsa path:
- `simlin_sim_new` LTM path: migrated in Phase 2
- `try_detect_ltm_loops`: migrated in Task 1
- `run_ltm_pipeline`: migrated in Task 2
- `simlin-cli simulate()`: migrated in Task 4

Delete both methods from `engine::Project`.

Also delete `abort_if_arrayed()` and `inject_ltm_vars()` helper functions if they have no other callers.

**Note:** Test-only callers in `tests/simulate_ltm.rs`, `ltm_augment.rs`, and `ltm.rs` still use these methods. Either:
- Keep the methods but mark them `#[cfg(test)]` for test-only use, OR
- Migrate the test callers to use the salsa path (preferred for Phase 6)

For this task, mark them `#[cfg(test)]` so tests keep passing. Phase 6 migrates the test callers.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io`
Expected: All tests pass. The methods are still available for tests via `#[cfg(test)]`.

**Commit:** `engine: eliminate with_ltm and with_ltm_all_links from production paths`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Migrate simlin-cli to incremental compilation path

**Verifies:** salsa-consolidation.AC5.1

**Files:**
- Modify: `src/simlin-cli/src/main.rs` (run_engine_project at line 199, finish_engine_project at line 217, simulate at line 240)

**Implementation:**

Replace `run_engine_project` (which uses `Simulation::new().compile()`) with a function that creates a `SimlinDb` and uses the incremental path:

```rust
fn run_incremental(datamodel: &datamodel::Project, model_name: &str, ltm: bool) -> StdResult<Results, Error> {
    let mut db = engine::db::SimlinDb::default();
    let sync = engine::db::sync_from_datamodel_incremental(&mut db, datamodel, None);
    if ltm {
        sync.project.set_ltm_enabled(&mut db).to(true);
    }
    let compiled = engine::db::compile_project_incremental(&db, sync.project, model_name)?;
    let mut vm = Vm::new(compiled)?;
    vm.run_to_end()?;
    Ok(vm.into_results())
}
```

Update `finish_engine_project` and `simulate` to call `run_incremental` instead of `run_engine_project`. The LTM path in `simulate()` no longer needs `engine_project.clone().with_ltm()` -- just pass `ltm: true` to `run_incremental`.

Delete `compile_simulation` from `src/libsimlin/src/lib.rs` (line 555) since it's no longer called from any production path after Phases 4 and 5 (AC3.3). Before deleting, verify all callers are migrated by searching: `grep -rn compile_simulation src/libsimlin/src/ --include='*.rs'` should return only the definition itself (and possibly test-only callers which should also be migrated or removed).

The three known callers that must be migrated before deletion:
1. `simulation.rs:95` (LTM path) -- migrated in Phase 2
2. `patch.rs:370` (simulatability check) -- migrated in Phase 4
3. `project.rs:604` (simlin_project_is_simulatable) -- migrated in Phase 4 Task 1c

**Verification:**
Run: `cargo test -p simlin-cli && cargo test -p simlin-engine --features file_io && cargo build -p simlin-mcp`
Expected: CLI produces identical simulation outputs. All tests pass. MCP compiles.

**Commit:** `cli: migrate to incremental compilation path`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Verification tests for internal caller migration

**Verifies:** salsa-consolidation.AC3.3, salsa-consolidation.AC5.1, salsa-consolidation.AC5.2, salsa-consolidation.AC5.3

**Files:**
- Test reference: `src/simlin-engine/tests/simulate.rs`
- Test reference: `src/simlin-engine/tests/simulate_ltm.rs`
- Test reference: `src/simlin-engine/tests/layout.rs`
- Test reference: `src/libsimlin/src/tests_incremental.rs`

**Testing:**

Tests must verify each AC listed above:
- **salsa-consolidation.AC3.3:** Verify `compile_simulation` no longer exists in libsimlin. Search the codebase for the function definition and confirm it's deleted. If any caller still references it, the build will fail (which is the verification).

- **salsa-consolidation.AC5.1:** Run the full simulation test suite (`cargo test -p simlin-engine --features file_io -- simulate`). All models must produce identical numerical results.

- **salsa-consolidation.AC5.2:** Run the LTM test suite (`cargo test -p simlin-engine --features file_io -- simulate_ltm`). All LTM models must produce identical scores.

- **salsa-consolidation.AC5.3:** Run libsimlin integration tests (`cargo test -p libsimlin`). All FFI tests including patch, analysis, and layout must pass.

No new tests needed for this task -- the existing test suite provides full coverage. The critical verification is that all existing tests continue to pass after the migration.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io && cargo test -p libsimlin && cargo test -p simlin-cli && cargo build -p simlin-mcp`
Expected: All tests pass across all three crates. simlin-mcp compiles.

**Commit:** `engine: verify all internal callers migrated to incremental path`
<!-- END_TASK_5 -->
