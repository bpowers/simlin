# Finish Salsa Migration Implementation Plan

**Goal:** Make the incremental salsa compilation pipeline the sole compilation path, then delete the monolithic code.

**Architecture:** The salsa-based incremental pipeline (`compile_project_incremental` / `assemble_simulation`) is already the production default. This plan migrates remaining callers, tests, and infrastructure, then deletes the monolithic path (`Project::from`, `Simulation::compile`, `compile_project`).

**Tech Stack:** Rust, salsa (incremental computation framework)

**Scope:** 7 phases from original design (phases 1-7)

**Codebase verified:** 2026-03-06

---

## Acceptance Criteria Coverage

This phase implements:

### finish-salsa-migration.AC7: All existing tests pass
- **finish-salsa-migration.AC7.1 Success:** All tests in `tests/simulate*.rs` pass with identical numerical results.
- **finish-salsa-migration.AC7.2 Success:** All LTM tests pass with identical results.
- **finish-salsa-migration.AC7.3 Success:** All libsimlin integration tests pass.
- **finish-salsa-migration.AC7.4 Success:** `cargo test -p simlin-engine` and `cargo test -p libsimlin` both pass cleanly.

---

## Phase 5: Migrate Tests to Incremental Path

**Phase type:** Functionality -- migrating all test code from monolithic to incremental compilation.

**Prerequisites:** Phase 4 (all production callers migrated).

**Migration scope:** The goal is zero test callers of `Simulation::compile()` or the `compile_project` free function, and zero test callers of `Project::from` (aliased as `CompiledProject::from` in some files) outside the retained interpreter path. `Simulation::new` + `run_to_end()` (the AST interpreter) are retained for cross-validation per AC4.6 and will be addressed separately in Phase 6.

**Note on line numbers:** Line numbers referenced throughout this phase are approximate and will shift as earlier phases modify files. Use function/test name search rather than relying on exact line numbers.

**Key pattern for migration:**

The monolithic test compilation pattern (using `Project::from` directly or via `CompiledProject::from` alias):
```rust
let project = Project::from(datamodel);        // also seen as CompiledProject::from(datamodel)
let sim = Simulation::new(&project, "main")?;
let compiled = sim.compile()?;
let mut vm = Vm::new(compiled)?;
```

Becomes the incremental pattern:
```rust
let mut db = SimlinDb::default();
let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
let compiled = compile_project_incremental(&db, sync.project, "main")?;
let mut vm = Vm::new(compiled)?;
```

**Important:** `TestProject::compile()` uses `CompiledProject::from(datamodel)` (which is `project::Project::from`), NOT `Simulation::compile()`. Only `TestProject::run_vm()` calls `sim.compile()` (via `build_sim()` then `sim.compile()`). This distinction matters because migrating `TestProject::compile()` requires replacing `Project::from`, while migrating `TestProject::run_vm()` requires replacing both `Project::from` and `sim.compile()`.

**Test file inventory (from codebase investigation):**

| File | Monolithic sites | Migration complexity |
|------|-----------------|---------------------|
| `test_common.rs` (TestProject) | 4 `CompiledProject::from` + 1 `sim.compile()` | High -- all other tests depend on this |
| `tests/simulate.rs` | ~20 `Project::from`, ~10 `sim.compile()` | High -- many helpers and inline tests |
| `tests/simulate_ltm.rs` | ~10 `Project::from` + `with_ltm` | High -- `with_ltm()` interleaved |
| `tests/vm_alloc.rs` | 1 `Project::from` + 1 `sim.compile()` | Low |
| `vm.rs` tests | 2 direct `Project::from` + 17 `sim.compile()` | Medium -- many test modules use `sim.compile()` |
| `compiler/symbolic.rs` tests | 2 `Project::from` | Low |
| `ltm.rs` tests | 23 `Project::from` | Medium -- graph APIs need compiled project |
| `ltm_augment.rs` tests | 3 `Project::from` + 5 `with_ltm_all_links()` | Medium -- also needs LTM method migration |
| `project.rs` tests | 3 `Project::from` + `with_ltm()` | Medium -- tests the `with_ltm()` method itself |
| `libsimlin/errors.rs` tests | 2 `Project::from` | Low |
| `vdf.rs` tests | 8 `Project::from` | Medium -- VDF binary format tests |
| `tests/roundtrip.rs` | 1 `Project::from` | Low |
| `model.rs` tests | 1 `Project::from` | Low |
| `db_diagnostic_tests.rs` | 1 `CompiledProject::from` | Low |
| `db_fragment_cache_tests.rs` | 1 `Project::from` | Low |
| `interpreter.rs` tests | 7 `Project::from` | N/A -- retained for AC4.6 cross-validation |
| `db_tests.rs` | 1 `Project::from` (line ~5804) | N/A -- retained for AC4.6 cross-validation |
| `unit_checking_test.rs` | 2 `CompiledProject::from` (direct, not via TestProject) | Low |

**Note:** `compiler/dimensions.rs` has a `#[cfg(test)]` block but uses no monolithic compilation paths -- no migration needed.

**Note:** `serde.rs` contains `project_io::Project::from(project.clone())` which is a *different* `From` impl (protobuf conversion, not engine compilation) and is NOT a migration target.

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Add incremental compilation methods to TestProject

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/test_common.rs`

**Implementation:**

Add incremental counterparts to the existing monolithic methods on `TestProject`. These new methods create a `SimlinDb`, sync the datamodel, and use `compile_project_incremental`.

Add these methods:

1. **`compile_incremental(&self) -> Result<CompiledSimulation, ...>`**: Creates a `SimlinDb`, calls `sync_from_datamodel_incremental`, then `compile_project_incremental`. Returns the `CompiledSimulation`.

2. **`run_vm_incremental(&self) -> HashMap<String, Vec<f64>>`**: Calls `compile_incremental`, creates a `Vm`, runs `run_to_end`, collects results.

3. **`assert_compiles_incremental(&self)`**: Asserts `compile_incremental` succeeds.

4. **`vm_result_incremental(&self, var: &str) -> Vec<f64>`**: Returns a single variable's VM results via the incremental path.

5. **`assert_vm_result_incremental(&self, var: &str, expected: &[f64])`**: Asserts a variable's incremental VM results match expected values.

Follow the pattern in `tests/simulate.rs::compile_vm` (line ~119) for the incremental compilation setup. The `SimlinDb` and sync state can be created fresh per call since these are test helpers.

Keep the existing monolithic methods (`compile`, `run_vm`, etc.) -- they will be removed in Phase 6 after all callers are migrated.

**Verification:**
```bash
cargo check -p simlin-engine
```
Expected: Compiles. New methods exist alongside old ones.

**Commit:** `engine: add incremental compilation methods to TestProject`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Migrate TestProject callers in unit test files

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/array_tests.rs`
- Modify: `src/simlin-engine/src/unit_checking_test.rs`

**Implementation:**

These files use `TestProject` methods like `assert_compiles()`, `assert_vm_result()`, `run_vm()`, etc. Migrate each call to its incremental counterpart:
- `assert_compiles()` -> `assert_compiles_incremental()`
- `assert_vm_result(var, expected)` -> `assert_vm_result_incremental(var, expected)`
- `run_vm()` -> `run_vm_incremental()`

For tests that use `assert_interpreter_result` or `run_interpreter`: these use `Simulation::new` + `run_to_end()` which is the AST interpreter path (retained per AC4.6). Keep these as-is for now.

**Note:** `unit_checking_test.rs` also has 2 direct `CompiledProject::from` calls (lines ~809, ~874) that are NOT mediated through TestProject. These must be individually migrated to `sync_from_datamodel_incremental` + `compile_project_incremental`.

**Verification:**
```bash
cargo test -p simlin-engine array_tests
cargo test -p simlin-engine unit_checking_test
```
Expected: All tests pass with identical results.

**Commit:** `engine: migrate array and unit-checking tests to incremental path`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->

<!-- START_TASK_3 -->
### Task 3: Migrate tests/simulate.rs VM compilation to incremental path

**Verifies:** finish-salsa-migration.AC7.1

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs`

**Implementation:**

1. **Delete `compile_vm_monolithic`** (line ~127). All models that currently use it as fallback (`delay.xmile`, `initial.xmile`, `smooth.xmile`, `wrld3-03.mdl`) should now work with the incremental `compile_vm`. If any still fail, investigate and fix the incremental path (this is a blocker -- do not keep monolithic fallbacks).

   **Known incremental path failures** (from `INCREMENTAL_BUILTIN_ISSUES` at simulate.rs:917 and wrld3-03 NaN at simulate.rs:1160):
   - `delay.xmile`: Incorrect initial values for subscripted DELAY builtins
   - `initial.xmile`: INIT builtin issue
   - `smooth.xmile`: Incorrect initial values for subscripted SMOOTH builtins
   - `wrld3-03.mdl`: NaN values for some variables in incremental compilation

   These must be fixed in the incremental compiler before the monolithic fallback can be deleted. Also note that `simulate_ltm.rs:642` documents a known limitation where LTM with module-containing models is not supported on the incremental path.

2. **Migrate `simulate_path_with` VM leg** (line ~287): The VM leg already uses the passed `compile` function which defaults to `compile_vm` (incremental). No change needed for the VM leg.

3. **Migrate `simulate_path_with` interpreter leg** (line ~301): Keep the interpreter leg using `Project::from` + `Simulation::new` + `run_to_end()` -- the AST interpreter is retained per AC4.6.

4. **Migrate `simulate_mdl_path`** (line ~419): Currently uses `Project::from` + `Simulation::new` + `sim.compile()` for the VM leg. Replace the VM compilation with `compile_vm` (the incremental function). Keep the interpreter leg.

5. **Migrate `simulate_mdl_path_interpreter_only`** (line ~455): Uses only the interpreter. Keep as-is.

6. **Migrate `simulate_mdl_path_with_data`** (line ~480): Same pattern as `simulate_mdl_path`. Replace VM compilation with incremental. Keep interpreter leg.

7. **Migrate inline tests** that directly call `sim.compile()`:
   - `simulates_except_basic_mdl` (line ~683)
   - `simulates_wrld3_03` (line ~1141)
   - `simulates_get_direct_data_scalar_csv` (line ~1364)
   - `simulates_get_direct_constants_scalar_csv` (line ~1438)
   - `simulates_get_direct_lookups_scalar_csv` (line ~1487)

   For each, replace the `sim.compile()` call with `compile_vm(&datamodel_project)`.

8. **Migrate `bad_model_name` test** (line ~1077): Tests that `Simulation::new` returns `Err` for bad model names. Keep the interpreter path test as-is.

**Verification:**
```bash
cargo test -p simlin-engine --test simulate
```
Expected: All tests pass (ignored tests remain ignored).

**Commit:** `engine: migrate simulate.rs VM tests to incremental compilation`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Migrate tests/simulate_ltm.rs to incremental path

**Verifies:** finish-salsa-migration.AC7.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm.rs`

**Implementation:**

LTM tests are complex because they use `with_ltm()` / `with_ltm_all_links()` which are monolithic `Project` methods. The incremental path handles LTM via:
- `set_project_ltm_enabled(&mut db, source_project, true)` -- enables LTM variable generation
- `model_detected_loops(db, source_model)` -- structural loop detection via salsa

Migration approach:

1. **Create a helper `compile_ltm_incremental`**: Creates a `SimlinDb`, syncs, enables LTM, compiles via `compile_project_incremental`, returns `CompiledSimulation`. Pattern:
   ```rust
   fn compile_ltm_incremental(project: &datamodel::Project) -> CompiledSimulation {
       let mut db = SimlinDb::default();
       let sync = sync_from_datamodel_incremental(&mut db, project, None);
       set_project_ltm_enabled(&mut db, sync.project, true);
       compile_project_incremental(&db, sync.project, "main").unwrap()
   }
   ```

2. **Migrate `simulate_ltm_path`** (line ~198): Replace `Project::from` + `with_ltm()` + `Simulation::new` + `run_to_end()` (interpreter leg) and `compile_project_incremental` (VM leg) with the unified incremental helper. The interpreter cross-validation leg can be kept using the monolithic path.

3. **Migrate `discover_loops_from_path`** (line ~246): Replace `Project::from` + `with_ltm_all_links()` with a helper that uses `set_project_ltm_enabled` + `model_detected_loops` for structural discovery.

4. **Migrate `TestProject`-based LTM tests**: Tests like `test_smooth_with_initial_value_ltm`, `test_smooth_goal_seeking_ltm` etc. use `TestProject::compile()` then `with_ltm()`. Add a `TestProject::compile_ltm_incremental()` helper that syncs to a `SimlinDb`, enables LTM, and compiles incrementally.

5. **Handle `discovery_*` tests** (lines ~302, ~357, ~420): These call `Project::from` for the exhaustive-search side of cross-validation. Keep the exhaustive side using the monolithic path (it will be removed with `Project::from` in Phase 6). Migrate the incremental side.

**Verification:**
```bash
cargo test -p simlin-engine --test simulate_ltm
```
Expected: All LTM tests pass with identical results.

**Commit:** `engine: migrate LTM simulation tests to incremental path`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 5-7) -->

<!-- START_TASK_5 -->
### Task 5: Migrate tests/vm_alloc.rs and vm.rs tests

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/tests/vm_alloc.rs`
- Modify: `src/simlin-engine/src/vm.rs` (test blocks)

**Implementation:**

1. **vm_alloc.rs**: Replace `build_scalar_model` helper (line ~62) to use `SimlinDb` + `sync_from_datamodel_incremental` + `compile_project_incremental` instead of `Project::from` + `Simulation::new` + `sim.compile()`. The `Vm::new` call stays the same -- only the compilation changes.

2. **vm.rs -- all 17 `sim.compile()` sites**: There are 17 `sim.compile()` call sites across multiple test modules in `vm.rs`. Key groups:
   - `per_variable_initials_tests`: `build_compiled` helper (line ~2915) uses `TestProject::build_sim()` + `sim.compile()`. Replace with `TestProject::compile_incremental()`. Also `test_per_var_initials_with_module` (line ~2977) uses direct `Project::from` + `sim.compile()` with file-loaded model.
   - `vm_reset_and_run_initials_tests` (line ~3132): Uses `TestProject::build_sim()` + `sim.compile()`.
   - `set_value_tests` (line ~3477): Multiple tests use `sim.compile()`. Includes `test_set_value_module_stock_returns_error` (line ~3765) with direct file-loaded `Project::from` + `sim.compile()`.
   - `vm_reset_run_to_and_constants_tests` (line ~4025): Uses `sim.compile()`.
   - `superinstruction_tests` (line ~4712): Uses `sim.compile()`.

   For all sites: replace `TestProject::build_sim()` + `sim.compile()` with `TestProject::compile_incremental()`, and replace direct `Project::from` + `sim.compile()` with incremental compilation via `SimlinDb`.

3. **vm.rs -- 2 direct `Project::from` file-load sites**: `test_per_var_initials_with_module` (line ~2977) and `test_set_value_module_stock_returns_error` (line ~3765) both load `modules_hares_and_foxes.stmx` via `Project::from`. Replace with incremental compilation.

**Verification:**
```bash
cargo test -p simlin-engine --test vm_alloc
cargo test -p simlin-engine vm::per_variable_initials_tests
cargo test -p simlin-engine vm::set_value_tests
```
Expected: All tests pass.

**Commit:** `engine: migrate vm_alloc and vm.rs tests to incremental compilation`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Migrate compiler/symbolic.rs tests

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/compiler/symbolic.rs` (test block starting at line ~1346)

**Implementation:**

The `compile_and_roundtrip` helper (line ~1830) uses `Project::from` to get a compiled `Module`, then tests symbolize/resolve roundtrip. Replace with incremental compilation to get the same compiled data.

The symbolize/resolve tests access `Module` internals (runlists, bytecode). Verify that the `CompiledSimulation` from `compile_project_incremental` provides the same compiled module data that these tests inspect. The compiled simulation contains `CompiledModule` objects with bytecode -- adapt the test helper to extract the needed data from the incremental output.

Also check the test at line ~2050 which uses `module.compile()` -- this may need `Module::new` (retained) rather than `Simulation::compile` (being removed).

**Verification:**
```bash
cargo test -p simlin-engine compiler::symbolic::tests
```
Expected: All tests pass.

**Commit:** `engine: migrate symbolic compiler tests to incremental path`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Migrate ltm.rs and ltm_augment.rs tests

**Verifies:** finish-salsa-migration.AC7.2

**Files:**
- Modify: `src/simlin-engine/src/ltm.rs` (test block at line ~1447)
- Modify: `src/simlin-engine/src/ltm_augment.rs` (test block at line ~824)

**Implementation:**

These tests use `Project::from` to get a compiled `Project` for graph analysis and LTM augmentation. The key APIs they test:
- `detect_loops(project)` -- needs compiled project for dependency graph
- `CausalGraph::new(...)` -- needs compiled model data
- `generate_ltm_variables(project, ...)` -- needs compiled project
- `compute_cycle_partitions(...)` -- graph algorithm, may not need compilation

Migration approach:

1. **For `detect_loops` tests** (~13 tests in ltm.rs): Replace `Project::from` with `SimlinDb` + `sync_from_datamodel_incremental`. Use `model_detected_loops(db, source_model)` (the salsa tracked function) instead of `detect_loops(&project)`. Compare results.

2. **For `CausalGraph` tests** (~8 tests): Use `model_causal_edges(db, source_model, source_project)` to get the causal graph data from salsa. Adapt assertions to use salsa output types.

3. **For `generate_ltm_variables` tests** (~15 tests in ltm_augment.rs): These tests call `generate_ltm_variables(&project, ...)` which operates on the monolithic `Project`. The incremental path's equivalent is the `ltm_augment_model` tracked function or similar. Investigate the incremental LTM augmentation path and adapt tests to use it.

4. **For TestProject-based tests** in ltm_augment.rs (~9 tests): Migrate from `TestProject::compile()` to `TestProject::compile_incremental()`.

5. **Keep pure algorithm tests unchanged**: Tests in ltm.rs that don't use `Project::from` (polarity detection, path analysis, etc.) need no changes.

**Verification:**
```bash
cargo test -p simlin-engine ltm::tests
cargo test -p simlin-engine ltm_augment::tests
```
Expected: All LTM tests pass.

**Commit:** `engine: migrate ltm and ltm_augment tests to incremental path`
<!-- END_TASK_7 -->

<!-- END_SUBCOMPONENT_C -->

<!-- START_SUBCOMPONENT_D (tasks 8-10) -->

<!-- START_TASK_8 -->
### Task 8: Migrate project.rs and libsimlin/errors.rs tests

**Verifies:** finish-salsa-migration.AC7.3, finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/project.rs` (test block at line ~421)
- Modify: `src/libsimlin/src/errors.rs` (test block at line ~410)

**Implementation:**

1. **project.rs tests** (7 tests): These test `with_ltm()` / `with_ltm_all_links()` behavior. Since these methods are gated to `#[cfg(test)]` in Phase 4, the tests can continue using them during this phase. But the underlying compilation should migrate from `Project::from` to incremental.

   For tests that call `Project::from` directly (3 tests at lines ~441, ~557, ~604): Replace with `SimlinDb` + `sync_from_datamodel_incremental`. Then use the salsa LTM functions to verify the same properties that `with_ltm()` tests currently check.

   For tests that call `TestProject::compile()` then `with_ltm()` (4 tests): Migrate to use `TestProject::compile_incremental()` and the salsa LTM path instead.

2. **libsimlin/errors.rs tests** (2 tests at lines ~421 and ~452): These call `Project::from` to get a compiled project with errors, then test error formatting via `collect_formatted_issues`. Replace `Project::from` with `SimlinDb` + `sync_from_datamodel_incremental` + `compile_project_incremental`. Use `collect_all_diagnostics` (the salsa accumulator path) to get errors. Adapt assertions to use the diagnostic format from the incremental path.

**Verification:**
```bash
cargo test -p simlin-engine project::tests
cargo test -p libsimlin errors::tests
```
Expected: All tests pass.

**Commit:** `engine: migrate project and error tests to incremental path`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Migrate remaining test files (vdf.rs, roundtrip.rs, model.rs, db_diagnostic_tests.rs, db_fragment_cache_tests.rs)

**Verifies:** finish-salsa-migration.AC7.4

**Files:**
- Modify: `src/simlin-engine/src/vdf.rs` (8 `Project::from` sites in tests at lines 2889, 3022, 3076, 3121, 3155, 3179, 3195, 3530)
- Modify: `src/simlin-engine/tests/roundtrip.rs` (1 `Project::from` at line 59)
- Modify: `src/simlin-engine/src/model.rs` (1 `Project::from` at line 1682 in test code)
- Modify: `src/simlin-engine/src/db_diagnostic_tests.rs` (1 `CompiledProject::from` at line 231)
- Modify: `src/simlin-engine/src/db_fragment_cache_tests.rs` (1 `Project::from` at line 861)

**Implementation:**

1. **vdf.rs** (8 sites): All test functions that parse VDF binary format use `Project::from` to get a compiled project for cross-validation. Replace each with `SimlinDb` + `sync_from_datamodel_incremental` + `compile_project_incremental`. The VDF tests need the compiled simulation's variable layout (offsets) -- verify that `compile_project_incremental` provides equivalent offset data.

2. **roundtrip.rs** (1 site): The XMILE roundtrip test calls `Project::from` to verify that parsing then reserializing a model produces no compilation errors. Replace with incremental compilation and use `collect_all_diagnostics` to check for errors.

3. **model.rs** (1 site at line 1682): Test code within `#[cfg(test)]`. Replace `Project::from` with incremental compilation.

4. **db_diagnostic_tests.rs** (1 site at line 231): Uses `CompiledProject::from(project.clone())`. Replace with incremental compilation.

5. **db_fragment_cache_tests.rs** (1 site at line 861): Uses `Project::from`. Replace with incremental compilation.

**Verification:**
```bash
cargo test -p simlin-engine vdf
cargo test -p simlin-engine --test roundtrip
cargo test -p simlin-engine model::tests
cargo test -p simlin-engine db_diagnostic_tests
cargo test -p simlin-engine db_fragment_cache_tests
```
Expected: All tests pass.

**Commit:** `engine: migrate vdf, roundtrip, and remaining test files to incremental path`
<!-- END_TASK_9 -->

<!-- START_TASK_10 -->
### Task 10: Full test suite verification

**Verifies:** finish-salsa-migration.AC7.1, finish-salsa-migration.AC7.2, finish-salsa-migration.AC7.3, finish-salsa-migration.AC7.4

**Step 1: Run all engine tests**

```bash
cargo test -p simlin-engine
```

**Step 2: Run all libsimlin tests**

```bash
cargo test -p libsimlin
```

**Step 3: Verify no remaining Simulation::compile() callers**

Search for `sim.compile()` and `Simulation::compile` in test code:
```bash
grep -rn "sim\.compile\(\)\|Simulation::compile" src/simlin-engine/tests/ src/simlin-engine/src/ --include="*.rs"
```

Expected: Zero matches (or only in the function definition itself, not callers).

**Step 4: Verify no remaining compile_project callers in tests**

```bash
grep -rn "compile_project\b" src/simlin-engine/tests/ --include="*.rs" | grep -v "compile_project_incremental"
```

Expected: Zero matches.

**Step 5: Verify remaining Project::from callers are only retained AC4.6 sites**

```bash
grep -rn "Project::from\|CompiledProject::from" src/simlin-engine/ --include="*.rs" | grep -v "compile_project_incremental\|sync_from_datamodel\|project_io::Project::from"
```

Expected: Only these retained sites (interpreter cross-validation per AC4.6):
- `interpreter.rs` test code (~7 sites at lines 2529, 2791, 2841, 2879, 2928, 2964, 3003)
- `db_tests.rs:5804` (1 site, interpreter cross-validation)
- Production sites in `patch.rs` and `db.rs` should already be migrated by Phase 4 Task 7

All other `Project::from` / `CompiledProject::from` callers should be migrated to the incremental path.

**Commit:** (no commit -- verification only)
<!-- END_TASK_10 -->

<!-- END_SUBCOMPONENT_D -->
