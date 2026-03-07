# Finish Salsa Migration Implementation Plan

**Goal:** Make the incremental salsa compilation pipeline the sole compilation path, then delete the monolithic code.

**Architecture:** The salsa-based incremental pipeline (`compile_project_incremental` / `assemble_simulation`) is already the production default. This plan migrates remaining callers, tests, and infrastructure, then deletes the monolithic path (`Project::from`, `Simulation::compile`, `compile_project`).

**Tech Stack:** Rust, salsa (incremental computation framework)

**Scope:** 7 phases from original design (phases 1-7)

**Codebase verified:** 2026-03-06

---

## Acceptance Criteria Coverage

This phase implements:

### finish-salsa-migration.AC4: Monolithic compilation path removed
- **finish-salsa-migration.AC4.1 Success:** `compile_project` (free function in interpreter.rs) does not exist.
- **finish-salsa-migration.AC4.2 Success:** `Simulation::compile()` does not exist.
- **finish-salsa-migration.AC4.3 Success:** `set_dependencies_cached`, `set_dependencies`, `all_deps` do not exist in model.rs.
- **finish-salsa-migration.AC4.4 Success:** `Project::from` impl and `Project::base_from` do not exist.
- **finish-salsa-migration.AC4.5 Success:** Legacy `errors`/`unit_errors` fields removed from `Variable`, `ModelStage0`, `ModelStage1`.
- **finish-salsa-migration.AC4.6 Success:** `Simulation::new()` + `run_to_end()` (AST interpreter) still works for cross-validation.

### finish-salsa-migration.AC5: Dependency analysis routed through salsa
- **finish-salsa-migration.AC5.1 Success:** All dependency analysis in the compilation pipeline goes through `variable_direct_dependencies` and `model_dependency_graph` tracked functions.
- **finish-salsa-migration.AC5.2 Success:** No production or test code calls `all_deps` or `set_dependencies`.

### finish-salsa-migration.AC6: Single sync path
- **finish-salsa-migration.AC6.1 Success:** No production caller invokes `sync_from_datamodel` directly; all go through `sync_from_datamodel_incremental`.
- **finish-salsa-migration.AC6.2 Success:** `sync_from_datamodel` remains as an internal bootstrap function called by `sync_from_datamodel_incremental` when `prev_state` is `None`.

---

## Phase 6: Delete Monolithic Compilation Path

**Phase type:** Infrastructure/deletion -- removing dead code after Phase 5 migrated all callers.

**Note on line numbers:** Line numbers referenced below are approximate and may shift as earlier phases modify files. Use function name search rather than relying on exact line numbers.

**Prerequisites:** Phase 5 (all test callers migrated to incremental path).

**Important codebase constraints discovered during investigation:**

1. **`calc_flattened_offsets` (interpreter.rs:2176) CANNOT be deleted.** It is called by `Simulation::new` (line 1841, retained per AC4.6) and by `vdf.rs` (lines 608, 972, 3036, 3198). Retain this function.

2. **`build_metadata` (compiler/mod.rs:2255) CANNOT be deleted.** It is called by `Module::new` (compiler/mod.rs:2424, retained per design). Retain this function.

3. **`Project::from` deletion (AC4.4) creates tension with interpreter retention (AC4.6).** `Simulation::new` takes `&Project` which is constructed by `Project::from`. Resolution options:
   - **Option A:** Gate `Project::from` / `base_from` with `#[cfg(test)]` so only the interpreter cross-validation tests can use it. This doesn't fully satisfy AC4.4 but preserves AC4.6.
   - **Option B:** Create a new `Project::from_salsa(db, source_project)` constructor that builds a `Project` from salsa query results, then delete the datamodel-based `Project::from`. This fully satisfies both ACs.
   - **Option C:** Refactor `Simulation::new` to accept salsa data directly instead of a monolithic `Project`.

   The executor should evaluate which option is most practical. Option A is simplest; Option B preserves the interpreter interface while removing the monolithic construction path.

4. **`Project::from_with_model_cb` does not exist** (design anticipated it but it was never created). `Project::base_from` serves that role.

5. **`compile_simulation` and `collect_project_errors` do not exist** in the codebase.

6. **`src/simlin-engine/src/patch.rs:317` uses `CompiledProject::from`** in `apply_rename_variable` for dependency-aware renaming. This production caller is migrated to the incremental path in Phase 4 Task 7. (Note: `src/libsimlin/src/patch.rs` has no such calls -- the engine and libsimlin each have their own `patch.rs`.)

7. **`serde.rs:2310` uses `project_io::Project::from`** which is a *different* `From` impl (protobuf conversion, not engine compilation). This is NOT affected by deleting the engine `Project::from<datamodel::Project>`.

8. **`db.rs:1889` (`get_stdlib_composite_ports`) uses `Project::from`** for static stdlib port computation inside a `OnceLock`. This production caller is migrated to the incremental path in Phase 4 Task 7.

9. **`Module::new` reads `ModelStage1.errors` at `compiler/mod.rs:2418`** for early-exit validation. Since `Module::new` is retained (the interpreter depends on it), `ModelStage1.errors` cannot be fully removed. The `Variable`-level `errors`/`unit_errors` fields are written during parsing and read by accessor methods -- they can be removed if the `Module::new` early-exit check is the only consumer of `ModelStage1.errors` (it can be simplified to check compilation result instead).

<!-- START_TASK_1 -->
### Task 1: Delete Simulation::compile() and compile_project

**Verifies:** finish-salsa-migration.AC4.1, finish-salsa-migration.AC4.2

**Files:**
- Modify: `src/simlin-engine/src/interpreter.rs`
  - Delete `Simulation::compile` (line ~1886) and its deprecation comment
  - Delete `compile_project` free function (line ~2102) and its deprecation comment
  - Retain `Simulation::new` (line ~1784) and `run_to_end` -- these are the AST interpreter
  - Retain `calc_flattened_offsets` (line ~2176) -- used by `Simulation::new` and `vdf.rs`
- Modify: `src/simlin-engine/benches/compiler.rs`
  - Delete `bench_project_build` function (line ~123) -- it benchmarks `CompiledProject::from` which is the monolithic path being removed. The `bench_bytecode_compile` function (line ~75) already benchmarks the incremental path.

**Implementation:**

Delete the two interpreter functions and the `bench_project_build` benchmark. Remove any `pub` exports from `mod.rs` or `lib.rs` that expose them. Check `src/simlin-engine/src/lib.rs` for re-exports.

After deletion, search for any remaining references:
```bash
grep -rn "Simulation::compile\|compile_project\b" src/ --include="*.rs" | grep -v "compile_project_incremental"
```

Fix any remaining references (should be none after Phase 5 migration).

**Verification:**
```bash
cargo check -p simlin-engine
```
Expected: Compiles without errors (no callers remain after Phase 5).

**Commit:** `engine: delete Simulation::compile and compile_project`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Delete dependency analysis functions from model.rs

**Verifies:** finish-salsa-migration.AC4.3, finish-salsa-migration.AC5.1, finish-salsa-migration.AC5.2

**Files:**
- Modify: `src/simlin-engine/src/model.rs`
  - Delete `set_dependencies_cached` (line ~1255)
  - Delete `set_dependencies` (line ~1094)
  - Delete `all_deps` (line ~302) and `all_deps_inner` (line ~319)
  - Delete any `build_runlist` closures within the deleted functions

**Implementation:**

These functions are called only from `Project::base_from` (project.rs:338, 347) which is being deleted in Task 3. Delete all four functions.

After deletion, check for any remaining callers:
```bash
grep -rn "set_dependencies_cached\|set_dependencies\|all_deps\b" src/ --include="*.rs"
```

Expected: Zero matches (except possibly comments in db_tests.rs or design docs).

**Verification:**
```bash
cargo check -p simlin-engine
```
Expected: Compiles without errors.

**Commit:** `engine: delete monolithic dependency analysis functions`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Delete Project::from and Project::base_from

**Verifies:** finish-salsa-migration.AC4.4, finish-salsa-migration.AC4.6

**Files:**
- Modify: `src/simlin-engine/src/project.rs`
  - Delete `From<datamodel::Project> for Project` impl (line ~120)
  - Delete `Project::base_from` (line ~177)
  - Delete `ModelStage0::new_cached` from `src/simlin-engine/src/model.rs` (line ~938)

**Implementation:**

**Prerequisite check:** Before proceeding, verify that zero production callers of `Project::from` / `CompiledProject::from` remain. Phase 4 Task 7 migrates `patch.rs:317` and `db.rs:1889`. Run `grep -rn 'Project::from\|CompiledProject::from' src/simlin-engine/src/ --include='*.rs' | grep -v '#\[cfg(test)\]\|mod tests\|test_\|_test\.rs\|_tests\.rs'` and confirm only test code remains.

This is the most architecturally significant deletion. Choose one of these approaches:

**Approach A (simplest): Gate with `#[cfg(test)]`**

Instead of deleting `Project::from` and `base_from`, gate them with `#[cfg(test)]`. This preserves the interpreter cross-validation path in tests while ensuring no production code can use them. This approach technically violates AC4.4 (the functions still exist in test builds) but satisfies the spirit of the requirement (no production path uses them).

**Approach B (full deletion): Create `Project::from_salsa`**

Create a new constructor `Project::from_salsa(db: &dyn Db, source_project: SourceProject, model_name: &str) -> Self` that builds a `Project` by reading salsa query results. This provides the `&Project` that `Simulation::new` needs without going through the monolithic `Project::from`. Then fully delete `Project::from` and `base_from`.

**For either approach:**
1. Also delete `ModelStage0::new_cached` (model.rs:938) which is only called from `base_from`
2. Remove any re-exports in `lib.rs`
3. Clean up unused imports

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles and all tests pass.

**Commit:** `engine: remove monolithic Project::from construction path`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Remove legacy error fields (TD17)

**Verifies:** finish-salsa-migration.AC4.5

**Files:**
- Modify: `src/simlin-engine/src/variable.rs`
  - Remove `errors: Vec<EquationError>` from `Variable::Stock` (line ~85), `Variable::Var` (line ~98), `Variable::Module` (line ~107)
  - Remove `unit_errors: Vec<EquationError>` from same variants (lines ~86, ~99, ~108)
- Modify: `src/simlin-engine/src/model.rs`
  - Remove `errors: Option<Vec<Error>>` from `ModelStage0` (line ~40)
  - Remove `errors: Option<Vec<Error>>` from `ModelStage1` (line ~56)
  - Remove accessor methods `get_variable_errors()`, `get_unit_errors()` if they exist

**Implementation:**

These fields are written during parsing (variable.rs) and read by:
- `model.rs` accessor methods
- `test_common.rs::compile()` and `assert_unit_error()`
- `libsimlin/errors.rs::collect_formatted_issues`
- `Module::new` (compiler/mod.rs:2418) for early-exit validation

**Migration approach:**

1. For `Variable` error fields: These are populated during parsing regardless of which compilation path is used. The incremental path uses `CompilationDiagnostic` accumulators instead. Since the `Variable` type is used by both paths, and `Module::new` (retained) reads `ModelStage1.errors`, evaluate whether the retained interpreter path needs these fields.

   If the interpreter needs them: Keep the fields but mark them `#[deprecated]`.
   If the interpreter doesn't need them: Delete the fields and update all constructors/pattern-matches.

2. For `ModelStage0.errors` and `ModelStage1.errors`: Same analysis. `Module::new` checks `self.errors` at compiler/mod.rs:2418 for early-exit. If `Module::new` is retained, this field may need to stay on `ModelStage1`.

**Pre-analysis of `Module::new` dependency:** `Module::new` at `compiler/mod.rs:2418` checks `model.errors` for early-exit validation. Since `Module::new` is retained (the interpreter depends on it per AC4.6), `ModelStage1.errors` must either be retained or the check in `Module::new` must be replaced with a different validation mechanism.

**Recommended approach:**
1. **Remove `Variable`-level `errors`/`unit_errors` fields** from all three enum variants. These are only consumed by accessor methods (`get_variable_errors`, `get_unit_errors`) which feed into the legacy `collect_formatted_issues` (being deleted in this phase) and `test_common.rs` error assertion methods (being deleted in this phase).
2. **Retain `ModelStage0.errors` and `ModelStage1.errors`** for now, since `Module::new` depends on them. Mark them `#[deprecated]` with a comment explaining they are retained only for the AST interpreter path.
3. Optionally, simplify `Module::new`'s early-exit check to use a simpler mechanism (e.g., a boolean flag) in a follow-up.

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles and all tests pass.

**Commit:** `engine: remove legacy error fields from Variable and ModelStage types`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Delete collect_formatted_issues and monolithic TestProject methods

**Verifies:** finish-salsa-migration.AC4.5

**Files:**
- Modify: `src/libsimlin/src/errors.rs`
  - Delete `collect_formatted_issues` (line ~70)
- Modify: `src/simlin-engine/src/test_common.rs`
  - Delete `compile()` (line ~371) -- the monolithic variant
  - Delete `build_sim()` (line ~420) -- uses `Project::from`
  - Delete `assert_unit_error()` (line ~540) -- uses `Project::from`
  - Delete `build_module()` (line ~697) -- uses `Project::from`
  - Delete `run_vm()` (line ~726) -- uses `sim.compile()`
  - Delete downstream methods that only delegate to deleted methods: `assert_compiles()`, `assert_compile_error()`, `run_interpreter()`, `assert_scalar_result()`, `assert_interpreter_result()`, `interpreter_result()`, `vm_result()`, `assert_vm_result()`, `assert_sim_builds()`, `flow_runlist_has_assign_temp()`
  - Retain `build_datamodel()` and all builder methods

**Implementation:**

After Phase 5, no test callers should reference the deleted methods. Delete them and verify no compile errors.

For `collect_formatted_issues`: the CLI was migrated in Phase 4 to use `collect_all_diagnostics`. The function at libsimlin/errors.rs:70 should have zero remaining callers (its own tests were migrated in Phase 5). Delete it.

**Note on `run_interpreter` and interpreter-dependent methods:** If the interpreter (`Simulation::new` + `run_to_end`) is retained, consider keeping `build_sim()` and `run_interpreter()` as incremental-path equivalents. If Approach A was used in Task 3 (`#[cfg(test)]` gating), then `build_sim` can stay since it calls `Project::from` which is now test-only.

**Verification:**
```bash
cargo check -p simlin-engine
cargo check -p libsimlin
cargo test -p simlin-engine
cargo test -p libsimlin
```
Expected: Compiles with no dead-code warnings for deleted functions. All tests pass.

**Commit:** `engine: delete monolithic TestProject methods and collect_formatted_issues`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Clean up dead code and stale references

**Files:**
- Search entire `src/simlin-engine/` and `src/libsimlin/` for stale references

**Step 1: Check for dead code warnings**

```bash
cargo build -p simlin-engine 2>&1 | grep "warning.*dead_code\|warning.*unused"
```

Delete any newly-dead code exposed by the Phase 6 deletions.

**Step 2: Remove stale docstrings and comments**

Search for references to deleted functions:
```bash
grep -rn "compile_project\b\|Simulation::compile\|set_dependencies\|all_deps\|collect_formatted_issues\|Project::from\b" src/ --include="*.rs" | grep -v "compile_project_incremental\|sync_from_datamodel"
```

Update or remove stale comments/docstrings that reference deleted code.

**Step 3: Verify clean build**

```bash
cargo build -p simlin-engine
cargo build -p libsimlin
cargo build -p simlin-cli
```

Expected: No warnings, no errors.

**Commit:** `engine: clean up dead code and stale references after monolithic path removal`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Full test suite verification

**Verifies:** finish-salsa-migration.AC4, finish-salsa-migration.AC5, finish-salsa-migration.AC6

**Step 1: Run all tests**

```bash
cargo test -p simlin-engine
cargo test -p libsimlin
cargo test -p simlin-cli
```

Expected: All tests pass.

**Step 2: Verify AC5 (dependency analysis through salsa)**

```bash
grep -rn "all_deps\|set_dependencies" src/ --include="*.rs"
```

Expected: Zero matches (functions deleted).

**Step 3: Verify AC6 (single sync path)**

```bash
grep -rn "sync_from_datamodel\b" src/ --include="*.rs" | grep -v "sync_from_datamodel_incremental\|// \|/// \|//! "
```

Expected: Only the function definition itself and calls from within `sync_from_datamodel_incremental`.

**Commit:** `engine: verify monolithic path fully removed

Fix #294`
<!-- END_TASK_7 -->
