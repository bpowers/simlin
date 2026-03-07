# Finish Salsa Migration Implementation Plan

**Goal:** Make the incremental salsa compilation pipeline the sole compilation path, then delete the monolithic code.

**Architecture:** The salsa-based incremental pipeline (`compile_project_incremental` / `assemble_simulation`) is already the production default. This plan migrates remaining callers, tests, and infrastructure, then deletes the monolithic path (`Project::from`, `Simulation::compile`, `compile_project`).

**Tech Stack:** Rust, salsa (incremental computation framework)

**Scope:** 7 phases from original design (phases 1-7)

**Codebase verified:** 2026-03-06

---

## Acceptance Criteria Coverage

This phase partially implements:

### finish-salsa-migration.AC4: Monolithic compilation path removed
- **finish-salsa-migration.AC4.4 Success:** `Project::from` impl and `Project::base_from` do not exist.

(AC4.1-AC4.3, AC4.5-AC4.6 are completed in Phases 5-6 after test migration and code deletion.)

### finish-salsa-migration.AC7: All existing tests pass
- **finish-salsa-migration.AC7.4 Success:** `cargo test -p simlin-engine` and `cargo test -p libsimlin` both pass cleanly.

---

## Phase 4: Migrate Remaining Monolithic Callers

**Phase type:** Functionality -- migrating production callers from monolithic to incremental compilation.

**Note on line numbers:** Line numbers referenced below are approximate and may shift as earlier phases modify files. Use function name search rather than relying on exact line numbers.

**Prerequisites:** Phases 2 (catch_unwind removal from incremental paths) and 3 (parse context unification) are complete.

**Overview of monolithic callers to migrate:**

| Caller | File | Line | Uses |
|--------|------|------|------|
| `try_compile_model` | layout/mod.rs | 1868 | `Project::from` for AST dep extraction |
| `try_detect_ltm_loops_monolithic` | layout/mod.rs | 2118 | Full monolithic LTM pipeline |
| `try_detect_ltm_loops` dispatcher | layout/mod.rs | 2013 | Falls back to monolithic when no db_state |
| `run_ltm_pipeline` | analysis.rs | 97 | `Project::from` for structural `discover_loops` |
| `run_datamodel_with_errors` | simlin-cli main.rs | 222 | `Project::from` for error reporting |
| `simulate` (LTM path) | simlin-cli main.rs | 234 | `Project::from` for `detect_loops` |
| `--equations` mode | simlin-cli main.rs | 332 | `Project::from` for variable iteration |
| `apply_rename_variable` | patch.rs | 317 | `CompiledProject::from` for dependency-aware renaming |
| `get_stdlib_composite_ports` | db.rs | 1889 | `Project::from` for static stdlib port computation |

**Note:** `simlin-mcp` calls `analyze_model` (analysis.rs:65) from two locations -- these will need updating when `analyze_model`'s signature changes.

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Make db_state required in layout and delete monolithic fallbacks

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/layout/mod.rs`
  - Delete `try_compile_model` (line ~1868)
  - Delete `try_detect_ltm_loops_monolithic` (line ~2118)
  - Modify `try_detect_ltm_loops` (line ~2013) to require db_state
  - Modify `compute_metadata` (line ~2260) to require db_state
  - Modify `generate_best_layout` (line ~2610) and `generate_layout` (line ~2580) signatures
  - Remove `catch_unwind` at lines 1874 and 2129 (deferred from Phase 2)

**Implementation:**

1. **Change `try_detect_ltm_loops` signature:** Replace `db_state: Option<(&mut SimlinDb, SourceProject)>` with `db: &mut SimlinDb, source_project: SourceProject`. Remove the `match db_state` dispatch -- always call `try_detect_ltm_loops_incremental`. Delete `try_detect_ltm_loops_monolithic` entirely.

2. **Change `compute_metadata` signature:** Replace `db_state: Option<(&mut SimlinDb, SourceProject)>` with `db: &mut SimlinDb, source_project: SourceProject`. For dependency extraction (currently done by `try_compile_model` + `Project::from`), use the salsa path instead: call `compile_var_fragment` or `variable_direct_dependencies` to get deps, rather than building a monolithic `Project`. Delete `try_compile_model` entirely. When the salsa compilation fails for a variable, fall back to the string heuristic `extract_equation_deps` (the same fallback that currently triggers when `try_compile_model` returns `None`).

3. **Update `generate_best_layout` and `generate_layout`:** Change the `db_state` parameter from `Option<...>` to required `(&mut SimlinDb, SourceProject)`. This is safe because libsimlin (the only production caller) always has db_state available.

4. **Remove remaining monolithic catch_unwind sites:** Lines 1874 (inside deleted `try_compile_model`) and 2129 (inside deleted `try_detect_ltm_loops_monolithic`) are removed by deletion.

**Testing:**

Tests should verify that layout generation still works correctly with the incremental-only path. Run:
```bash
cargo test -p simlin-engine --test layout
cargo test -p simlin-engine layout
```

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles and all tests pass.

**Commit:** `engine: make salsa db required for layout, delete monolithic fallbacks`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update libsimlin layout callers for required db_state

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/libsimlin/src/layout.rs` (line ~101, `simlin_project_diagram_sync`)

**Implementation:**

The current code obtains `db_state` as `Option<(&mut SimlinDb, SourceProject)>` by mapping over `sync_state`. Since Phase 4 makes `db_state` required, update the call site:

1. If `sync_state` is `None` (project not yet synced), either return early with an error/empty layout, or sync first then proceed. The design intent is that layout generation always happens after the project is synced, so an early return with an appropriate error is correct.

2. If `sync_state` is `Some`, extract `db` and `source_project` and pass them directly (no longer wrapped in `Option`).

**Verification:**
```bash
cargo check -p libsimlin
cargo test -p libsimlin
```
Expected: Compiles and all tests pass.

**Commit:** `libsimlin: update layout callers for required salsa db`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Migrate analyze_model to accept salsa db

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/analysis.rs`
  - Change `analyze_model` signature (line ~65)
  - Change `run_ltm_pipeline` (line ~97) to accept db + source_project
  - Remove `catch_unwind` at line ~124
  - Remove `Project::from` usage in `run_ltm_pipeline` (line ~150)

**Implementation:**

1. **Change `analyze_model` signature** from `(project: &datamodel::Project, model_name: &str)` to `(db: &mut SimlinDb, source_project: SourceProject, model_name: &str)`. Remove the internal `SimlinDb::default()` creation -- use the caller's db.

2. **Change `run_ltm_pipeline`** similarly. It currently creates its own `SimlinDb` and uses `Project::from` only for the structural `discover_loops` call. Replace that `Project::from` usage with the salsa equivalent: `model_detected_loops(db, source_model)` or `model_causal_edges(db, source_model, source_project)` -- whichever provides the structural loop information that `discover_loops` currently computes from the monolithic `Project`.

3. **Remove `catch_unwind`** at line ~124. Since the incremental path returns clean `Result::Err`, propagate errors through `Result` / `Option` instead.

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles (callers will need updating in next tasks).

**Commit:** `engine: migrate analyze_model to salsa db`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Update simlin-mcp callers of analyze_model

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-mcp/src/tools/edit_model.rs` (line ~225)
- Modify: `src/simlin-mcp/src/tools/read_model.rs` (line ~58)

**Implementation:**

Both callers need to provide a `&mut SimlinDb` and `SourceProject` to the updated `analyze_model`. Investigate how these MCP tools currently obtain the `datamodel::Project` and whether they have access to a salsa db. If they construct a `datamodel::Project` from scratch, they will need to sync it to a `SimlinDb` first via `sync_from_datamodel_incremental`.

Follow the pattern used by libsimlin: lock the shared db, obtain source_project from sync_state, and pass both through.

**Verification:**
```bash
cargo check -p simlin-mcp
```
Expected: Compiles without errors.

**Commit:** `mcp: update analyze_model callers for salsa db`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-6) -->

<!-- START_TASK_5 -->
### Task 5: Migrate simlin-cli error reporting and --equations to incremental path

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-cli/src/main.rs`
  - `run_datamodel_with_errors` (line ~222): replace `Project::from` error reporting
  - `--equations` mode (line ~332): replace `Project::from` variable iteration
  - LTM path in `simulate` (line ~234): replace `Project::from` for loop detection

**Implementation:**

1. **Error reporting (`run_datamodel_with_errors`):** Currently calls `Project::from` then `collect_formatted_issues` to report errors. Replace with: create a `SimlinDb`, call `sync_from_datamodel_incremental`, then use `collect_all_diagnostics` (the salsa accumulator path) to gather errors. Format and display them. The `run_incremental` function already creates its own `SimlinDb` -- consider refactoring so both error reporting and simulation share the same db.

2. **`--equations` mode:** Currently calls `Project::from` to iterate `project.models` for LaTeX equation output. Replace with: sync to a `SimlinDb`, then iterate `SourceModel` variables via salsa queries. The variable equations are available via `SourceVariable::equation(db)`.

3. **LTM loop detection:** The `simulate` function builds a `Project::from` for `ltm::detect_loops`. Replace with the salsa equivalent `model_detected_loops(db, source_model)`. The `run_incremental` function already handles LTM-enabled compilation via `set_project_ltm_enabled`.

**Verification:**
```bash
cargo check -p simlin-cli
```
Expected: Compiles without errors.

**Commit:** `cli: migrate all compilation to incremental salsa path`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Verify CLI functionality end-to-end

**Files:**
- Read: test model files in `test/` directory

**Step 1: Test basic simulation**

```bash
cargo run -p simlin-cli -- simulate test/predator_prey/predator_prey.xmile
```

Expected: Outputs simulation results without errors.

**Step 2: Test error reporting**

```bash
cargo run -p simlin-cli -- simulate test/missing_var/missing_var.xmile 2>&1 || true
```

Expected: Reports errors cleanly, does not panic.

**Step 3: Test --equations mode (if it exists as a CLI flag)**

```bash
cargo run -p simlin-cli -- --help
```

Check available subcommands and test the equations output mode.

**Commit:** (no commit -- verification only)
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_C -->

<!-- START_TASK_7 -->
### Task 7: Migrate patch.rs and db.rs production callers of Project::from

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/patch.rs` (line ~317, `apply_rename_variable`)
- Modify: `src/simlin-engine/src/db.rs` (line ~1889, `get_stdlib_composite_ports`)

**Implementation:**

1. **`patch.rs:317` (`apply_rename_variable`):** This function calls `CompiledProject::from(project.clone())` to get a compiled model for dependency-aware variable renaming. Replace with salsa-based compilation: create a `SimlinDb`, sync via `sync_from_datamodel_incremental`, then use `variable_direct_dependencies` or `compile_project_incremental` to get the dependency information needed for renaming. The caller already has the `datamodel::Project` available.

2. **`db.rs:1889` (`get_stdlib_composite_ports`):** This function uses `Project::from(dm_project)` inside a `OnceLock` to compute static stdlib composite port information that never changes. Since stdlib models are static constants, two migration approaches are valid:
   - **Option A:** Use the incremental path with a temporary `SimlinDb` created inside the `OnceLock` closure (same pattern as the current code but using incremental compilation).
   - **Option B:** Keep `Project::from` as an explicit exception gated behind a private helper, since the stdlib models are compile-time constants. Document the exception clearly.

   Option A is preferred for consistency. The `OnceLock` ensures this runs at most once per process.

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles and all tests pass.

**Commit:** `engine: migrate patch.rs and db.rs production callers to incremental path`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Delete with_ltm() and with_ltm_all_links() from project.rs

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/project.rs`
  - Delete `with_ltm()` (line ~48)
  - Delete `with_ltm_all_links()` (line ~74)

**Implementation:**

Both methods have zero production callers after deleting `try_detect_ltm_loops_monolithic` in Task 1. The only production caller of `with_ltm()` was `try_detect_ltm_loops_monolithic` (layout/mod.rs:2140). `with_ltm_all_links()` had zero production callers.

However, test callers still exist across many files (`simulate_ltm.rs`, `ltm.rs`, `project.rs`, `ltm_augment.rs`) — deleting the methods outright will cause compilation failures.

**Approach: Gate with `#[cfg(test)]`.** Move `with_ltm()` and `with_ltm_all_links()` inside a `#[cfg(test)]` block so they remain available to tests but are removed from production builds. Phase 5 will migrate the test callers to the incremental path, and Phase 6 will delete them entirely.

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles (with #[cfg(test)] gate if needed) and all tests pass.

**Commit:** `engine: remove with_ltm production paths, gate test-only usage

Fix #292`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Full test suite verification

**Step 1: Run all engine tests**

```bash
cargo test -p simlin-engine
```

**Step 2: Run all libsimlin tests**

```bash
cargo test -p libsimlin
```

**Step 3: Run CLI tests if any exist**

```bash
cargo test -p simlin-cli
```

Expected: All tests pass. No production code path calls `Project::from`, `Simulation::compile()`, `with_ltm()`, or `with_ltm_all_links()`.
<!-- END_TASK_9 -->
