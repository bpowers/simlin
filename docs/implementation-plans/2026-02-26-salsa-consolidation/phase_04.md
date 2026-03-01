# Salsa Consolidation Phase 4: Replace engine::Project in apply_patch

**Goal:** Remove `engine::Project` from `SimlinProject` and the `apply_patch` flow. All error checking goes through the salsa incremental path. `SimlinProject` stores `datamodel::Project` directly instead of the heavier `engine::Project`.

**Architecture:** `SimlinProject` changes its `project: Mutex<engine::Project>` field to `datamodel: Mutex<datamodel::Project>`. The `apply_patch` flow replaces `Project::from_with_salsa_sync` + `compile_simulation` with `compile_project_incremental` + `Vm::new` validation + `collect_all_diagnostics`. All error collection uses the accumulator path exclusively. The rollback/commit protocol adapts to work with `datamodel::Project` instead of `engine::Project`.

**Tech Stack:** Rust (libsimlin crate, simlin-engine)

**Scope:** Phase 4 of 6 from original design (depends on Phase 3)

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### salsa-consolidation.AC2: Patch error checking uses incremental path only
- **salsa-consolidation.AC2.1 Success:** `apply_patch` with a valid equation edit produces identical accept/reject decisions as the current dual-path implementation.
- **salsa-consolidation.AC2.6 Success:** `apply_patch` no longer calls `compile_simulation` (monolithic path eliminated).

---

<!-- START_SUBCOMPONENT_A (tasks 1a-1c) -->

<!-- START_TASK_1a -->
### Task 1a: Change SimlinProject struct and update simple datamodel consumers

**Verifies:** None (structural change; verified by compilation)

**Files:**
- Modify: `src/libsimlin/src/lib.rs` (SimlinProject struct at line 275, new_synced_db at line 315)
- Modify: `src/libsimlin/src/project.rs` (simlin_project_open_* functions, simlin_project_get_model_count, simlin_project_get_model_names, simlin_project_get_model, simlin_project_add_model)
- Modify: `src/libsimlin/src/serialization.rs` (all serialize functions)
- Modify: `src/libsimlin/src/layout.rs` (simlin_project_diagram_sync)
- Modify: `src/libsimlin/src/simulation.rs` (simlin_sim_new)

**Implementation:**

Change `SimlinProject` struct field from `project: Mutex<engine::Project>` to `datamodel: Mutex<datamodel::Project>`. Update `new_synced_db` to take `&datamodel::Project`.

Update the simple consumers that only read `project_locked.datamodel.*` -- these become `datamodel_locked.*`:
- **serialization.rs**: All serialize functions (protobuf, json, xmile, svg, png) read `project_locked.datamodel` for serialization. Change to `datamodel_locked` directly.
- **layout.rs**: Reads and mutates `project_locked.datamodel`. Change to `datamodel_locked`.
- **simulation.rs**: Already uses db for non-LTM. LTM path uses `project_locked.datamodel`. Change to `datamodel_locked`.
- **project.rs simple readers**: `get_model_count` reads `.datamodel.models.len()`, `get_model_names` reads `.datamodel.models`, `get_model` reads `.datamodel.models`. All become `datamodel_locked.*`.
- **project.rs simlin_project_add_model**: Change to modify `datamodel_locked.models` directly, then re-sync the db (instead of `engine::Project::from(...)`).
- **simlin_project_open_***: Store `datamodel::Project` directly instead of building `engine::Project`.

**Verification:**
Run: `cargo build -p libsimlin`
Expected: Compiles (some functions may still need `engine::Project` features that are addressed in Tasks 1b/1c -- use temporary `engine::Project::from()` calls as scaffolding if needed to keep compiling).

**Commit:** `libsimlin: change SimlinProject.project to datamodel::Project`
<!-- END_TASK_1a -->

<!-- START_TASK_1b -->
### Task 1b: Rewrite model.rs query functions to use salsa db

**Verifies:** None (structural change; verified by compilation and tests)

**Files:**
- Modify: `src/libsimlin/src/model.rs` (simlin_model_get_var_count, simlin_model_get_var_names, simlin_model_get_dependencies, simlin_model_get_links, simlin_model_get_equations, simlin_model_get_latex_equations, simlin_model_get_sim_specs)

**Implementation:**

Several model.rs functions currently use `engine::Project.models` (the `HashMap<Ident<Canonical>, Arc<ModelStage1>>`) for compiled model data. Rewrite each to use the salsa db:

- **get_dependencies**: Currently reads `ModelStage1.variables` for dependency info. Use `variable_direct_dependencies(db, var, model, project)` from the salsa pipeline.
- **get_links**: Currently reads `ModelStage1.variables` for causal links. Use `model_causal_edges(db, model, project)`.
- **get_equations**: Currently reads `Variable.equation()` from parsed variables. Use `parse_source_variable(db, var, model, project)` to get the parsed equation text.
- **get_latex_equations**: Same approach -- query salsa db for parsed equation data.
- **get_var_count, get_var_names**: These can read from `datamodel::Model.variables` directly (no compiled data needed).
- **get_sim_specs**: Read from `datamodel::Project.sim_specs` or `datamodel::Model.sim_specs` directly.

Each function locks `datamodel` for the model lookup, then locks `db` for salsa queries. The lock order must be consistent: always `datamodel` before `db` (or hold only one at a time).

**Verification:**
Run: `cargo test -p libsimlin`
Expected: All model query tests pass with identical results.

**Commit:** `libsimlin: rewrite model.rs query functions to use salsa db`
<!-- END_TASK_1b -->

<!-- START_TASK_1c -->
### Task 1c: Rewrite project.rs is_simulatable and get_errors to use incremental path

**Verifies:** salsa-consolidation.AC2.1

**Files:**
- Modify: `src/libsimlin/src/project.rs` (simlin_project_is_simulatable at line 603, simlin_project_get_errors at line 634)
- Modify: `src/libsimlin/src/analysis.rs` (simlin_analyze_get_loops, simlin_analyze_get_links)

**Implementation:**

- **simlin_project_is_simulatable**: Currently calls `compile_simulation(&project_locked, ...)`. Rewrite to lock the db, get the sync state, and call `compile_project_incremental(&db, project, "main")`. Return success/failure based on the Result.

- **simlin_project_get_errors**: Currently calls `gather_error_details_with_db(&project_locked, ...)`. Rewrite to use accumulator diagnostics: lock db, compile, collect diagnostics, format.

- **analysis.rs get_loops/get_links**: Currently uses `engine::Project` for causal graph. Rewrite to lock the db and use salsa tracked functions: `model_loop_circuits(db, model, project)` and `model_causal_edges(db, model, project)`.

**Verification:**
Run: `cargo test -p libsimlin -- simulatable`
Run: `cargo test -p libsimlin`
Expected: All tests pass with identical simulatability verdicts and error reports.

**Commit:** `libsimlin: rewrite is_simulatable, get_errors, and analysis to use incremental path`
<!-- END_TASK_1c -->

<!-- START_TASK_2 -->
### Task 2: Rewrite apply_project_patch_internal for incremental path

**Verifies:** salsa-consolidation.AC2.1, salsa-consolidation.AC2.6

**Files:**
- Modify: `src/libsimlin/src/patch.rs` (apply_project_patch_internal at line 449)

**Implementation:**

Rewrite the patch flow to eliminate `engine::Project` and `compile_simulation`:

```
1. Lock datamodel, clone for staged_datamodel and original_datamodel, release lock.
2. Apply engine::apply_patch(&mut staged_datamodel, patch). On failure, return error.
3. Lock db. Get prev_state from sync_state.
4. sync_from_datamodel_incremental(&mut db, &staged_datamodel, prev_state) -> staged_sync.
5. compile_project_incremental(&db, staged_sync.project, "main") -> compiled_result.
   This triggers the full salsa pipeline. Errors accumulate via CompilationDiagnostic.
6. If compilation succeeds: Vm::new(compiled) to validate bytecode.
7. collect_all_diagnostics(&db, &staged_sync) -> diagnostics.
8. Accept/reject based on diagnostics (see Task 4).
9. Rollback: sync_from_datamodel_incremental with original_datamodel.
   Commit: re-acquire locks in order, re-sync with staged_datamodel, update datamodel field.
```

The key difference from the current flow:
- No `Project::from_with_salsa_sync` call (no engine::Project construction)
- No `compile_simulation` call (monolithic path eliminated -- AC2.6)
- Error detection uses only `collect_all_diagnostics` (accumulator path)
- Rollback/commit protocol works with `datamodel::Project` instead of `engine::Project`

The rollback/commit protocol needs adaptation:
- Rollback: same pattern but writes to `*datamodel_locked` instead of `*project_locked`
- Commit: three-lock protocol writes `*datamodel_locked = staged_datamodel` instead of `*project_locked = staged_project`

**Verification:**
Run: `cargo test -p libsimlin`
Expected: All tests pass, including all patch acceptance tests in `tests_incremental.rs`. The accept/reject decisions must be identical to the current implementation (AC2.1).

**Commit:** `libsimlin: rewrite apply_patch to use incremental compilation path`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 2-3) -->

<!-- START_TASK_3 -->
### Task 3: Rewrite gather_error_details_with_db to use only accumulator diagnostics

**Verifies:** salsa-consolidation.AC2.1, salsa-consolidation.AC2.6

**Files:**
- Modify: `src/libsimlin/src/patch.rs` (gather_error_details_with_db at line 333)
- Modify: `src/libsimlin/src/errors.rs` (collect_formatted_issues at line 70)

**Implementation:**

Rewrite `gather_error_details_with_db` to use ONLY accumulator diagnostics:

```rust
pub(crate) fn gather_error_details_with_db(
    db: &engine::db::SimlinDb,
    sync: &engine::db::SyncResult<'_>,
    compiled: Option<&engine::CompiledSimulation>,
    vm_error: Option<&engine::Error>,
) -> Vec<ErrorDetailData> {
    // 1. Collect all accumulator diagnostics
    let diagnostics = engine::db::collect_all_diagnostics(db, sync);

    // 2. Convert diagnostics to ErrorDetailData
    let mut errors: Vec<ErrorDetailData> = diagnostics.iter()
        .map(|d| ErrorDetailBuilder::from_diagnostic(d))
        .collect();

    // 3. If VM validation failed, add that error
    if let Some(err) = vm_error {
        errors.push(ErrorDetailBuilder::from_vm_error(err));
    }

    errors
}
```

The function no longer takes `&engine::Project` or calls `collect_project_errors`. The struct-field error walking is eliminated.

Update `errors::collect_formatted_issues` in `src/libsimlin/src/errors.rs` to work with `Vec<Diagnostic>` instead of `&engine::Project`. Or replace it entirely with a new function `format_diagnostics(diagnostics: &[Diagnostic]) -> FormattedErrors` that converts accumulator diagnostics to the display format.

The existing `format_diagnostic` function at errors.rs line 326 already converts `Diagnostic` -> `FormattedError`, so the infrastructure exists.

**Verification:**
Run: `cargo test -p libsimlin`
Expected: All tests pass with identical error reporting.

**Commit:** `libsimlin: rewrite error collection to use only accumulator diagnostics`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Update rejection logic to use accumulator diagnostics

**Verifies:** salsa-consolidation.AC2.1

**Files:**
- Modify: `src/libsimlin/src/patch.rs` (first_error_code at line 386, rejection logic around line 545)

**Implementation:**

Rewrite `first_error_code` to operate on `Vec<Diagnostic>` instead of walking `engine::Project` struct fields:

```rust
fn first_error_code(diagnostics: &[Diagnostic]) -> Option<SimlinErrorCode> {
    for d in diagnostics {
        if d.severity == DiagnosticSeverity::Error {
            return Some(diagnostic_to_error_code(&d.error));
        }
    }
    None
}
```

Where `diagnostic_to_error_code` maps `DiagnosticError` variants to `SimlinErrorCode`:
- `DiagnosticError::Equation(e)` -> map `e.code` to SimlinErrorCode
- `DiagnosticError::Model(e)` -> map `e.code` to SimlinErrorCode
- `DiagnosticError::Unit(_)` -> `SimlinErrorCode::UnitDefinitionErrors`
- `DiagnosticError::Assembly(_)` -> `SimlinErrorCode::NotSimulatable`

For `new_unit_warning` detection: compare diagnostics with `severity: Warning` before and after the patch. If a model previously had no unit warnings and now has them, set `new_unit_warning`.

The `collect_models_with_unit_warnings` function (currently reads from engine::Project) needs updating to use diagnostics from the pre-patch compilation state.

**Verification:**
Run: `cargo test -p libsimlin`
Expected: All patch acceptance tests pass with identical accept/reject decisions (AC2.1).

**Commit:** `libsimlin: update patch rejection logic to use accumulator diagnostics`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Verification tests for apply_patch migration

**Verifies:** salsa-consolidation.AC2.1, salsa-consolidation.AC2.6

**Files:**
- Modify: `src/libsimlin/src/tests_incremental.rs` (existing acceptance tests)
- Modify: `src/libsimlin/src/tests_remaining.rs` (existing tests)

**Testing:**

Tests must verify each AC listed above:
- **salsa-consolidation.AC2.1:** The existing tests in `tests_incremental.rs` (labeled AC3.x) already verify patch acceptance behavior through the FFI. These tests should pass unchanged after this migration, confirming identical accept/reject decisions. Run them and verify all pass.

- **salsa-consolidation.AC2.6:** Add an assertion or test that verifies `compile_simulation` is not called during `apply_patch`. One approach: temporarily make `compile_simulation` panic if called, run the patch tests, and verify no panic occurs. Alternatively, verify at the code level that `compile_simulation` has no callers in the patch path.

Also verify:
- Patch with valid equation: accepted, simulation produces correct results
- Patch with syntax error: rejected with specific error code
- Patch with circular dependency: rejected with assembly error
- Patch rollback: after rejection, model state is unchanged
- Patch commit: after acceptance, model state reflects the patch

The existing test suite in `tests_incremental.rs` covers most of these scenarios. Focus on ensuring no regressions.

**Verification:**
Run: `cargo test -p libsimlin`
Expected: All new and existing tests pass.

**Commit:** `libsimlin: verify apply_patch uses only incremental compilation path`
<!-- END_TASK_5 -->
