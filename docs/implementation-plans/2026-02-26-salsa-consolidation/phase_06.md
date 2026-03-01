# Salsa Consolidation Phase 6: Delete Old Bytecode Compilation Path

**Goal:** Remove the monolithic bytecode compilation code. All bytecode generation goes through the salsa incremental pipeline. Tests and benchmarks are migrated to use `compile_project_incremental`. The AST interpreter (`Simulation::new` + `run_to_end`) is retained for cross-validation.

**Architecture:** With all production callers migrated (Phases 1-5), the monolithic compilation path (`compile_project`, `Simulation::compile()`, `Module::compile()`, `build_metadata`, `compile_simulation`, `calc_flattened_offsets`) is dead code. Tests are migrated from `sim.compile()` to `compile_project_incremental` for the VM leg, while retaining the interpreter leg via `Simulation::new` + `run_to_end`. Benchmarks switch to the incremental path. Struct-field error aggregation on `ModelStage1` is removed.

**Tech Stack:** Rust (simlin-engine, libsimlin crates)

**Scope:** Phase 6 of 6 from original design (depends on Phase 5)

**Codebase verified:** 2026-02-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### salsa-consolidation.AC3: Old bytecode compilation path deleted
- **salsa-consolidation.AC3.1 Success:** `compile_project` free function no longer exists in the codebase.
- **salsa-consolidation.AC3.2 Success:** `Simulation::compile()`, `Module::compile()`, and `build_metadata` no longer exist.
- **salsa-consolidation.AC3.4 Success:** All tests and benchmarks that previously used the monolithic bytecode path now use `compile_project_incremental` and produce identical results.
- **salsa-consolidation.AC3.5 Success:** Error struct fields on `ModelStage1` and `Variable` are removed. No struct-field error walking remains.

### salsa-consolidation.AC4: Interpreter preserved
- **salsa-consolidation.AC4.1 Success:** `Simulation::new()` + `Simulation::run_to_end()` still works for cross-validation in tests.
- **salsa-consolidation.AC4.2 Success:** `Module::new` still builds `Vec<Expr>` runlists for the interpreter.
- **salsa-consolidation.AC4.3 Success:** Interpreter results match VM results for all test models (existing cross-validation tests pass).

### salsa-consolidation.AC5: All existing tests pass
- **salsa-consolidation.AC5.1 Success:** All tests in `tests/simulate*.rs` pass with identical numerical results.
- **salsa-consolidation.AC5.2 Success:** All tests in `tests/simulate_ltm.rs` pass with identical numerical results.
- **salsa-consolidation.AC5.3 Success:** All libsimlin integration tests pass.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Migrate tests/simulate.rs to incremental VM path

**Verifies:** salsa-consolidation.AC3.4, salsa-consolidation.AC4.1, salsa-consolidation.AC4.3, salsa-consolidation.AC5.1

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (simulate_path helper at line 241, simulate_xmile helper)

**Implementation:**

The `simulate_path` helper currently uses `sim.compile()` for the VM leg. Change it to use `compile_project_incremental`:

Current pattern:
```rust
let project = Rc::new(Project::from(datamodel_project.clone()));
let sim = Simulation::new(&project, "main").unwrap();
let results1 = sim.run_to_end();       // interpreter leg (RETAIN)
let compiled = sim.compile().unwrap();  // monolithic VM compilation (REPLACE)
let mut vm = Vm::new(compiled).unwrap();
vm.run_to_end().unwrap();
```

New pattern:
```rust
let project = Rc::new(Project::from(datamodel_project.clone()));
let sim = Simulation::new(&project, "main").unwrap();
let results1 = sim.run_to_end();       // interpreter leg (RETAINED)

// VM leg via incremental path
let mut db = SimlinDb::default();
let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
let mut vm = Vm::new(compiled).unwrap();
vm.run_to_end().unwrap();
```

The interpreter leg (`Simulation::new` + `run_to_end`) is RETAINED for cross-validation. Only the VM compilation changes from monolithic to incremental.

Apply the same change to the protobuf roundtrip and XMILE roundtrip sections within `simulate_path` -- they also use `sim.compile()` and need to use `compile_project_incremental` instead.

Note: The existing test at line 806 (`incremental_compilation_covers_all_models`) already tests the incremental path. After this migration, ALL simulation tests use the incremental path for VM compilation.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io -- simulate`
Expected: All simulation tests pass with identical numerical results.

**Commit:** `engine: migrate simulate.rs tests to incremental VM path`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Migrate tests/simulate_ltm.rs to incremental VM path

**Verifies:** salsa-consolidation.AC3.4, salsa-consolidation.AC5.2

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm.rs` (simulate_ltm_path helper at line 197)

**Implementation:**

The `simulate_ltm_path` helper uses `sim.compile()` for VM compilation. Change to incremental:

Current pattern:
```rust
let sim = Simulation::new(&ltm_project, "main").unwrap();
let results1 = sim.run_to_end().unwrap();      // interpreter (RETAIN)
let compiled = sim.compile().unwrap();           // monolithic (REPLACE)
let mut vm = Vm::new(compiled).unwrap();
```

New pattern:
```rust
let sim = Simulation::new(&ltm_project, "main").unwrap();
let results1 = sim.run_to_end().unwrap();      // interpreter (RETAINED)

// VM leg via incremental path with LTM enabled
let mut db = SimlinDb::default();
let sync = sync_from_datamodel_incremental(&mut db, &ltm_project.datamodel, None);
sync.project.set_ltm_enabled(&mut db).to(true);
let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
let mut vm = Vm::new(compiled).unwrap();
```

The LTM test needs `ltm_enabled=true` on the SourceProject so the incremental path includes LTM synthetic variables. The interpreter leg still uses `with_ltm()` (which is `#[cfg(test)]` after Phase 5) for the reference results.

Also update the inline test at line 626 that uses `sim.compile()`.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io -- simulate_ltm`
Expected: All LTM tests pass with identical scores.

**Commit:** `engine: migrate simulate_ltm.rs tests to incremental VM path`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Migrate benchmarks to incremental path

**Verifies:** salsa-consolidation.AC3.4

**Files:**
- Modify: `src/simlin-engine/benches/compiler.rs` (bench_bytecode_compile at line 135, bench_full_pipeline at line 173, bench_incremental_equation_edit at line 260, bench_incremental_add_remove at line 311)
- Modify: `src/simlin-engine/benches/simulation.rs` (compile_population helper at line 24)
- Modify: `src/simlin-engine/benches/array_ops.rs` (local compile_project at line 265)

**Implementation:**

**compiler.rs:**
- `bench_bytecode_compile`: Replace `Simulation::new` + `sim.compile()` with `compile_project_incremental`. Create a `SimlinDb` in the bench setup.
- `bench_full_pipeline`: Same replacement.
- `bench_incremental_equation_edit` and `bench_incremental_add_remove`: Delete the monolithic baseline groups that use the `compile_project` free function (lines 281, 328, 350). Keep only the incremental groups.

**simulation.rs:**
- `compile_population`: Replace `Simulation::new` + `sim.compile()` with `compile_project_incremental`. The `SimlinDb` can be created in the setup and reused across iterations.

**array_ops.rs:**
- Replace the local `compile_project` helper with an incremental version:
  ```rust
  fn compile_project(datamodel: &datamodel::Project) -> Result<CompiledSimulation, String> {
      let mut db = SimlinDb::default();
      let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
      compile_project_incremental(&db, sync.project, "main")
          .map_err(|e| e.to_string())
  }
  ```

**Verification:**
Run: `cargo bench -p simlin-engine -- --test` (run benchmarks in test mode to verify they compile and execute)
Expected: All benchmarks compile and execute without errors.

**Commit:** `engine: migrate benchmarks to incremental compilation path`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->

<!-- START_TASK_4 -->
### Task 4: Delete monolithic bytecode compilation functions

**Verifies:** salsa-consolidation.AC3.1, salsa-consolidation.AC3.2

**Files:**
- Modify: `src/simlin-engine/src/interpreter.rs` (delete compile_project at line 1625, Simulation::compile at line 1422, calc_flattened_offsets at line 1699)
- Modify: `src/simlin-engine/src/compiler/mod.rs` (delete Module::compile at line 1227; RETAIN build_metadata at line 886)
- Modify: `src/simlin-engine/src/compiler/codegen.rs` (delete Compiler struct at line 26, Compiler::new at line 49, Compiler::compile at line 1224, and all Compiler methods)
- Modify: `src/simlin-engine/src/lib.rs` (remove `compile_project` from the re-export at line 74)

**Implementation:**

Delete the following functions/types:

1. **`compile_project`** (interpreter.rs:1625) -- the free function that drives monolithic compilation.

2. **`Simulation::compile()`** (interpreter.rs:1422) -- the method that calls Module::compile() for each module.

3. **`Module::compile()`** (compiler/mod.rs:1227) -- the one-liner that delegates to Compiler::new(self).compile().

4. **`build_metadata`** (compiler/mod.rs:886) -- **RETAIN, do not delete.** `Module::new` calls `build_metadata` at line 1055, and `Module::new` is retained for the AST interpreter path. `build_metadata` is interpreter infrastructure, not part of the monolithic bytecode compilation path being deleted.

5. **`Compiler` struct** (codegen.rs:26) and all its methods -- the monolithic bytecode compiler. This is the bulk of the deletion. Note: Check if any of the `Compiler` methods are used by the incremental path (compile_var_fragment in db.rs). The incremental path uses `compile_var_fragment` which has its own compilation logic -- verify there is no shared code.

6. **`calc_flattened_offsets`** (interpreter.rs:1699) -- the monolithic offset calculator. Note: `vdf.rs` at lines 608, 972, 3036, 3198 uses this function. Either migrate those call sites to `calc_flattened_offsets_incremental` or keep `calc_flattened_offsets` for VDF use only.

7. **`compile_simulation`** in libsimlin (lib.rs:555) -- should already be deleted in Phase 5 Task 4. Verify it's gone; if not, delete it here.

8. **`Project::from_with_salsa_sync`** (project.rs:161) -- the patch validation bridge between monolithic and salsa. After Phase 4 rewrites apply_patch, this has no production callers. Delete it (along with its one test).

Update the public re-export in `src/simlin-engine/src/lib.rs:74` to remove `compile_project`.

**Important: RETAIN these:**
- `Module::new` (compiler/mod.rs:1032) -- needed by `Simulation::new` for the interpreter
- `Simulation::new` + `run_to_end` (interpreter.rs:1330, 1547) -- retained for cross-validation
- `Opcode` enum and `ByteCode` (bytecode.rs) -- VM still uses these
- All `Expr` types and expression lowering -- interpreter needs them

**Verification:**
Run: `cargo build -p simlin-engine && cargo build -p libsimlin`
Expected: Compiles with no errors. All deleted functions have no remaining callers.

**Commit:** `engine: delete monolithic bytecode compilation path`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Remove struct-field error aggregation paths

**Verifies:** salsa-consolidation.AC3.5

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (model_all_diagnostics at line 1315)
- Modify: `src/simlin-engine/src/model.rs` (ModelStage1 errors/unit_warnings fields)
- Modify: `src/simlin-engine/src/project.rs` (Project.errors field, collect error methods)
- Modify: `src/libsimlin/src/patch.rs` (collect_project_errors, if still exists)
- Modify: `src/libsimlin/src/errors.rs` (collect_formatted_issues, if still references engine::Project)

**Implementation:**

After Phase 3 (error accumulator consolidation), the salsa accumulator is the sole error source. The struct-field error aggregation is redundant.

1. **Remove `model_all_diagnostics` struct-field sync** (db.rs:1315): The current function reads `Variable.equation_errors()` and `Variable.unit_errors()` to populate the accumulator. After Phase 3, `compile_var_fragment` accumulates errors directly and `check_model_units` accumulates unit errors directly. The `model_all_diagnostics` function can be simplified to just trigger the tracked functions that accumulate errors (compile_var_fragment, check_model_units) without reading struct fields.

2. **Remove `ModelStage1.errors` and `ModelStage1.unit_warnings`** (model.rs:50,53): After Phase 6, `Module::new` (which checked `model.errors`) is still retained but only for the interpreter path. The interpreter doesn't need to check these errors -- it can just attempt to run and fail if the model is invalid. Remove the fields and update `Module::new` to not check them.

3. **Remove `Project.errors`** (project.rs:25): Project-level errors are now accumulated. Remove the field.

4. **Remove `collect_project_errors`** (patch.rs:321) and `collect_formatted_issues` (errors.rs:70) if they still exist and walk struct fields. Phase 4 should have already replaced these.

5. **Variable.errors and Variable.unit_errors**: **RETAIN these fields.** They are populated during `lower_variable` (expression lowering) which the interpreter path depends on. `Module::new` -> expression lowering writes errors to these fields during `Variable` construction. Removing the fields would require changing the lowering function signatures, which is out of scope. Instead, remove only the `model_all_diagnostics` code that reads these fields to populate the salsa accumulator (since Phase 3's direct accumulation in `compile_var_fragment` and `check_model_units` makes that sync redundant).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io && cargo test -p libsimlin`
Expected: All tests pass. Error reporting is unchanged (same error codes and messages).

**Commit:** `engine: remove struct-field error walking paths`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Delete remaining monolithic test utilities and verify

**Verifies:** salsa-consolidation.AC3.1, salsa-consolidation.AC3.2, salsa-consolidation.AC3.4, salsa-consolidation.AC4.1, salsa-consolidation.AC4.2, salsa-consolidation.AC4.3, salsa-consolidation.AC5.1, salsa-consolidation.AC5.2, salsa-consolidation.AC5.3

**Files:**
- Modify: `src/simlin-engine/src/test_common.rs` (TestProject builder -- update compile/build_sim methods if they use monolithic path)
- Modify: `src/simlin-engine/src/db_tests.rs` (remove cross-validation tests that compared monolithic vs incremental)
- Modify: `src/simlin-engine/src/interpreter.rs` (remove unit tests that tested compile_project)

**Implementation:**

1. **Update TestProject** (test_common.rs): If `TestProject::compile()` or `TestProject::build_sim()` uses `Simulation::new().compile()` internally, update to use `compile_project_incremental`. The `run_interpreter()` method should continue to use `Simulation::new` + `run_to_end`. The `run_vm()` method should use `compile_project_incremental` + `Vm::new` + `run_to_end`.

2. **Clean up db_tests.rs**: Tests like `compile_project_f64_matches_simulation_compile` (which compared monolithic vs incremental output) are no longer needed since there's only one path. Remove these cross-validation tests. Keep tests that verify incremental behavior.

3. **Clean up interpreter.rs unit tests**: Tests like `compile_project_nonexistent_model_errors` that test the deleted `compile_project` function should be deleted or migrated to test the incremental path.

4. **Verify Module::new still works**: `Module::new` is retained for the interpreter. Ensure it still builds `Expr` runlists correctly. The existing `TestProject::run_interpreter()` tests verify this.

5. **Verify interpreter cross-validation**: The `simulate_path` helper (from Task 1) runs both interpreter and VM. Verify they still produce identical results for all test models.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io && cargo test -p libsimlin && cargo test -p simlin-cli`
Expected: All tests pass. No references to deleted functions remain. Interpreter results match VM results for all models.

**Commit:** `engine: clean up monolithic test utilities and verify consolidation`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_B -->
