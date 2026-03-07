# Test Requirements: finish-salsa-migration

Maps each acceptance criterion to automated tests or documented human verification. Rationalized against the implementation phases in `/home/bpowers/src/simlin/docs/implementation-plans/2026-03-06-finish-salsa-migration/`.

---

## finish-salsa-migration.AC1: Incremental pipeline handles all model types

### finish-salsa-migration.AC1.1: Models with module variables compile through incremental path with identical results

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Existing test | `test_incremental_compile_smooth_over_module_output` |
| Test file | `src/simlin-engine/src/db_tests.rs` |
| Phase | 1 (verification only -- test already exists at line ~4004) |
| Method | Syncs a model with module variables to `SimlinDb`, compiles via `compile_project_incremental`, runs VM, and asserts simulation results match expected values. |

Additionally verified by the broad integration test:

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Existing test | `incremental_compilation_covers_all_models` |
| Test file | `src/simlin-engine/tests/simulate.rs` |
| Phase | 1 (verification), 2 (catch_unwind removal) |
| Method | Iterates all test models and compiles each through `compile_project_incremental`. After Phase 2, uses `Result` matching instead of `catch_unwind`. |

### finish-salsa-migration.AC1.2: SMOOTH/DELAY/TREND builtins compile with correct layout slots for implicit variables

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Existing test | `test_incremental_compile_smooth_over_module_output` |
| Test file | `src/simlin-engine/src/db_tests.rs` |
| Phase | 1 (verification only) |
| Method | Creates a model using SMOOTH (which generates implicit stock variables). Compilation via `compile_project_incremental` succeeds only if `compute_layout` allocates slots for implicit variables. VM run confirms correct simulation output. |

### finish-salsa-migration.AC1.3: Multiple sub-model instances with different input wirings produce distinct compiled module entries

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Existing test | `test_incremental_compile_distinguishes_module_input_sets` |
| Test file | `src/simlin-engine/src/db_tests.rs` |
| Phase | 1 (verification only) |
| Method | Creates a model with multiple instances of the same sub-model wired with different inputs. Compiles via `compile_project_incremental` and asserts distinct compiled module entries. |

### finish-salsa-migration.AC1.4: Existing named tests pass

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Verification command | `cargo test -p simlin-engine test_incremental_compile_smooth_over_module_output test_incremental_compile_distinguishes_module_input_sets` |

---

## finish-salsa-migration.AC2: Incremental path never panics on malformed models

### finish-salsa-migration.AC2.1: Unknown builtins return Err(NotSimulatable), not panic

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Existing test | `incremental_compilation_covers_all_models` (C-LEARN model) |
| Test file | `src/simlin-engine/tests/simulate.rs` |
| Phase | 2, Task 2 |
| Method | After Phase 2 removes `catch_unwind`, models with unsupported Vensim macros must return `Err` from `compile_project_incremental` without panicking. A panic would be caught by the test harness as a test failure. |

### finish-salsa-migration.AC2.2: Missing module references return Err, not panic

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Existing test | `incremental_compilation_covers_all_models` |
| Test file | `src/simlin-engine/tests/simulate.rs` |
| Phase | 2, Task 2 |
| Method | Same mechanism as AC2.1. Models with missing module references produce `Err` from `compile_project_incremental`. Without `catch_unwind`, a panic would fail the test loudly. |

### finish-salsa-migration.AC2.3: catch_unwind wrappers removed from benchmarks, tests, and incremental layout paths

| Field | Value |
|-------|-------|
| Test type | Static analysis (grep) |
| Automated | Yes |
| Phase | 2 (incremental paths), 4 (monolithic paths) |
| Method | `grep -rn "catch_unwind" src/simlin-engine/ --include="*.rs"` returns zero matches after all phases complete. Phase 2 removes: `benches/compiler.rs:75`, `tests/simulate.rs:1294`, `layout/mod.rs:2041`, `layout/mod.rs:2061`. Phase 4 removes: `layout/mod.rs:1874`, `layout/mod.rs:2129`, `analysis.rs:124`. |

### finish-salsa-migration.AC2.4: compile_project_incremental docstring is accurate

| Field | Value |
|-------|-------|
| Test type | Human verification |
| Automated | No |
| Justification | Docstring accuracy is a semantic property that cannot be mechanically tested. |
| Verification approach | During Phase 2 code review, reviewer confirms the updated docstring on `compile_project_incremental` (in `src/simlin-engine/src/db.rs`) no longer claims a monolithic fallback. |

---

## finish-salsa-migration.AC3: Module-aware parse context unified

### finish-salsa-migration.AC3.1: Only parse_source_variable_with_module_context exists; plain variant deleted

| Field | Value |
|-------|-------|
| Test type | Static analysis (compilation) |
| Automated | Yes |
| Phase | 3, Tasks 2-3 |
| Method | After Phase 3 Task 3 deletes `parse_source_variable`, `cargo build -p simlin-engine` succeeds only if zero callers remain. Post-hoc grep: `grep -rn "parse_source_variable\b" src/simlin-engine/src/db.rs | grep -v "parse_source_variable_with_module_context\|parse_source_variable_impl"` returns zero results. |

### finish-salsa-migration.AC3.2: PREVIOUS(SMTH1_var) compiles to module expansion, not LoadPrev

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| New test | `test_previous_of_module_backed_variable_compiles_correctly` |
| Test file | `src/simlin-engine/src/db_tests.rs` |
| Phase | 3, Task 1 |
| Method | Creates a model with `x = SMTH1(input, 1)` and `y = PREVIOUS(x)`. Syncs to `SimlinDb`, compiles via `compile_project_incremental`, runs VM. Asserts `y` produces values consistent with PREVIOUS of a smoothed series (module expansion), not a raw one-timestep-shifted scalar (LoadPrev). |

### finish-salsa-migration.AC3.3: Editing unrelated variable does not trigger re-parse of other variables

| Field | Value |
|-------|-------|
| Test type | Design verification + optional unit test |
| Automated | Partially |
| Phase | 3, Task 4 |
| Method | Cache stability is structurally guaranteed by salsa's interning: `ModuleIdentContext` is `#[salsa::interned]` (db.rs:140), so unchanged module-ident sets produce the same interned ID. Optional test: sync model, parse var A, edit unrelated var B, re-sync, assert var A's parse result has same `salsa::Id`. |

---

## finish-salsa-migration.AC4: Monolithic compilation path removed

### finish-salsa-migration.AC4.1: compile_project (free function) does not exist

| Field | Value |
|-------|-------|
| Test type | Static analysis (compilation) |
| Automated | Yes |
| Phase | 6, Task 1 |
| Method | After deletion, `cargo build -p simlin-engine` succeeds. Post-hoc grep: `grep -rn "compile_project\b" src/ --include="*.rs" | grep -v "compile_project_incremental"` returns zero results (excluding comments). |

### finish-salsa-migration.AC4.2: Simulation::compile() does not exist

| Field | Value |
|-------|-------|
| Test type | Static analysis (compilation) |
| Automated | Yes |
| Phase | 6, Task 1 |
| Method | After deletion, `cargo build -p simlin-engine` succeeds. Post-hoc grep: `grep -rn "Simulation::compile\|sim\.compile()" src/ --include="*.rs"` returns zero results. |

### finish-salsa-migration.AC4.3: set_dependencies_cached, set_dependencies, all_deps do not exist

| Field | Value |
|-------|-------|
| Test type | Static analysis (compilation + grep) |
| Automated | Yes |
| Phase | 6, Task 2 |
| Method | After deletion, `grep -rn "set_dependencies_cached\|set_dependencies\|all_deps\b" src/ --include="*.rs"` returns zero results. |

### finish-salsa-migration.AC4.4: Project::from and Project::base_from do not exist (or are test-only)

| Field | Value |
|-------|-------|
| Test type | Static analysis (compilation + grep) |
| Automated | Yes |
| Phase | 6, Task 3 |
| Method | Under Approach A (#[cfg(test)]), `cargo build -p simlin-engine` in non-test mode must not export `Project::from`. Under Approach B (full deletion), the functions are gone entirely. Grep verification: `grep -rn "Project::from\|CompiledProject::from\|Project::base_from" src/ --include="*.rs" | grep -v "#\[cfg(test)\]\|project_io::Project::from\|sync_from_datamodel"` returns zero production results. |

### finish-salsa-migration.AC4.5: Legacy errors/unit_errors fields removed from Variable, ModelStage0, ModelStage1

| Field | Value |
|-------|-------|
| Test type | Static analysis (compilation + grep) |
| Automated | Yes |
| Phase | 6, Task 4 |
| Method | `grep -rn "errors.*Vec<EquationError>\|unit_errors.*Vec<EquationError>" src/simlin-engine/src/variable.rs` returns zero results. Note: `ModelStage1.errors` may be retained with `#[deprecated]` because `Module::new` (retained for interpreter) reads it. |

### finish-salsa-migration.AC4.6: Simulation::new() + run_to_end() still works for cross-validation

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Existing tests | ~7 tests in `interpreter.rs` test block, 1 in `db_tests.rs:~5804` |
| Phase | 6 (retained, not migrated) |
| Verification command | `cargo test -p simlin-engine interpreter` |
| Method | These tests use `Project::from` (via `#[cfg(test)]` or `Project::from_salsa`) then `Simulation::new` + `run_to_end()`. Explicitly retained per Phase 5 inventory. |

---

## finish-salsa-migration.AC5: Dependency analysis routed through salsa

### finish-salsa-migration.AC5.1: All dependency analysis goes through salsa tracked functions

| Field | Value |
|-------|-------|
| Test type | Static analysis (grep) + integration |
| Automated | Yes |
| Phase | 6, Task 2 |
| Method | After deletion, `grep -rn "all_deps\|set_dependencies" src/ --include="*.rs"` returns zero results. Full test suite passing (`cargo test -p simlin-engine`) implicitly verifies dependency analysis works through salsa tracked functions. |

### finish-salsa-migration.AC5.2: No code calls all_deps or set_dependencies

| Field | Value |
|-------|-------|
| Test type | Static analysis (grep) |
| Automated | Yes |
| Phase | 6, Task 7 |
| Method | `grep -rn "all_deps\b\|set_dependencies\b" src/ --include="*.rs"` returns zero results. |

---

## finish-salsa-migration.AC6: Single sync path

### finish-salsa-migration.AC6.1: No production caller invokes sync_from_datamodel directly

| Field | Value |
|-------|-------|
| Test type | Static analysis (grep) |
| Automated | Yes |
| Phase | 1 (verification), 6 Task 7 (final) |
| Method | `grep -rn "sync_from_datamodel\b" src/ --include="*.rs" | grep -v "sync_from_datamodel_incremental\|// \|/// \|//! "` returns only the function definition and calls from within `sync_from_datamodel_incremental`. |

### finish-salsa-migration.AC6.2: sync_from_datamodel remains as internal bootstrap

| Field | Value |
|-------|-------|
| Test type | Human verification + static analysis |
| Automated | Partially |
| Phase | 1 (verification) |
| Method | Phase 1 Task 2 verifies that `sync_from_datamodel_incremental` calls `sync_from_datamodel` when `prev_state` is `None`. Grep confirms the function still exists. The "internal bootstrap" semantic is verified by code review. |

---

## finish-salsa-migration.AC7: All existing tests pass

### finish-salsa-migration.AC7.1: All tests in tests/simulate*.rs pass with identical numerical results

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Phase | 5 (Tasks 3, 4), 6 Task 7 |
| Verification command | `cargo test -p simlin-engine --test simulate --test simulate_ltm` |

### finish-salsa-migration.AC7.2: All LTM tests pass with identical results

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Phase | 5 (Tasks 4, 7) |
| Verification command | `cargo test -p simlin-engine --test simulate_ltm && cargo test -p simlin-engine ltm && cargo test -p simlin-engine ltm_augment` |

### finish-salsa-migration.AC7.3: All libsimlin integration tests pass

| Field | Value |
|-------|-------|
| Test type | Integration |
| Automated | Yes |
| Phase | 4 Task 2, 5 Task 8, 6 Task 7 |
| Verification command | `cargo test -p libsimlin` |

### finish-salsa-migration.AC7.4: cargo test for simlin-engine and libsimlin pass cleanly

| Field | Value |
|-------|-------|
| Test type | Integration (full suite) |
| Automated | Yes |
| Phase | Every phase includes this as a final verification step |
| Verification command | `cargo test -p simlin-engine && cargo test -p libsimlin` |

---

## finish-salsa-migration.AC8: Dimension-granularity invalidation

### finish-salsa-migration.AC8.1: Changing dimension A does not trigger re-parse of a scalar variable

| Field | Value |
|-------|-------|
| Test type | Unit |
| Automated | Yes |
| New test | `test_dimension_invalidation_scalar_immune` |
| Test file | `src/simlin-engine/src/db_tests.rs` |
| Phase | 7, Task 4 |
| Method | Create model with dimensions A/B, scalar `x = 10`, arrayed `y[A] = x + 1`. Sync, parse `x`. Add element to dimension A. Re-sync. Assert `x`'s parse result is cached (same `salsa::Id`). |

### finish-salsa-migration.AC8.2: Changing dimension A does not trigger re-parse of a variable referencing only dimension B

| Field | Value |
|-------|-------|
| Test type | Unit |
| Automated | Yes |
| New test | `test_dimension_invalidation_cross_dimension_immune` |
| Test file | `src/simlin-engine/src/db_tests.rs` |
| Phase | 7, Task 4 |
| Method | Create model with dimensions A/B, variable `y[B] = 5`. Sync, parse `y`. Modify dimension A. Re-sync. Assert `y`'s parse result is cached. |

### finish-salsa-migration.AC8.3: Changing dimension A triggers re-parse of a variable referencing dimension A

| Field | Value |
|-------|-------|
| Test type | Unit |
| Automated | Yes |
| New tests | `test_dimension_invalidation_same_dimension_reparse`, `test_dimension_invalidation_maps_to_chain_reparse` |
| Test file | `src/simlin-engine/src/db_tests.rs` |
| Phase | 7, Task 4 |
| Method (direct) | Create model with dimensions A/B, variable `y[A] = 5`. Sync, parse `y`. Modify dimension A. Re-sync. Assert `y`'s parse result was re-computed. |
| Method (maps_to) | Create model where dimension A `maps_to` B, variable `y[A] = 5`. Sync, parse `y`. Modify dimension B. Re-sync. Assert `y` was re-parsed because `expand_maps_to_chains` includes B in the relevant set. |

---

## Summary: Coverage Matrix

| AC | Sub-criterion | Automated | Test type | Phase |
|----|--------------|-----------|-----------|-------|
| AC1.1 | Module vars compile incrementally | Yes | Integration | 1 |
| AC1.2 | SMOOTH/DELAY/TREND layout slots | Yes | Integration | 1 |
| AC1.3 | Distinct module instances | Yes | Integration | 1 |
| AC1.4 | Named tests pass | Yes | Integration | 1 |
| AC2.1 | Unknown builtins return Err | Yes | Integration | 2 |
| AC2.2 | Missing modules return Err | Yes | Integration | 2 |
| AC2.3 | catch_unwind removed | Yes | Static analysis | 2, 4 |
| AC2.4 | Docstring accurate | No | Human review | 2 |
| AC3.1 | Plain parse_source_variable deleted | Yes | Compilation | 3 |
| AC3.2 | PREVIOUS(SMTH1) module expansion | Yes | Integration | 3 |
| AC3.3 | Salsa cache stability | Partial | Design + optional unit | 3 |
| AC4.1 | compile_project deleted | Yes | Compilation | 6 |
| AC4.2 | Simulation::compile deleted | Yes | Compilation | 6 |
| AC4.3 | Dependency fns deleted | Yes | Compilation | 6 |
| AC4.4 | Project::from deleted/gated | Yes | Compilation | 6 |
| AC4.5 | Legacy error fields removed | Yes | Compilation | 6 |
| AC4.6 | AST interpreter retained | Yes | Integration | 6 |
| AC5.1 | Deps through salsa tracked fns | Yes | Static + integration | 6 |
| AC5.2 | No code calls all_deps/set_deps | Yes | Static analysis | 6 |
| AC6.1 | No direct sync_from_datamodel callers | Yes | Static analysis | 1, 6 |
| AC6.2 | sync_from_datamodel is bootstrap | Partial | Static + human | 1 |
| AC7.1 | simulate*.rs tests pass | Yes | Integration | 5, 6 |
| AC7.2 | LTM tests pass | Yes | Integration | 5 |
| AC7.3 | libsimlin tests pass | Yes | Integration | 4, 5, 6 |
| AC7.4 | Full cargo test passes | Yes | Integration | All |
| AC8.1 | Scalar immune to dim changes | Yes | Unit | 7 |
| AC8.2 | Cross-dimension immune | Yes | Unit | 7 |
| AC8.3 | Same-dimension triggers reparse | Yes | Unit | 7 |

---

## Human Verification Items

### 1. finish-salsa-migration.AC2.4: Docstring accuracy

**Justification:** Docstring correctness is a semantic judgment that cannot be mechanically tested.

**Verification approach:** During Phase 2 code review, confirm the updated docstring on `compile_project_incremental` in `src/simlin-engine/src/db.rs`:
1. No mention of "monolithic fallback" or "falls back to compile_project"
2. Accurately states this is the production compilation entry point
3. Accurately describes the error return type (`Err(NotSimulatable)`)

### 2. finish-salsa-migration.AC3.3: Salsa cache stability (partial)

**Justification:** Cache stability is guaranteed by salsa's interning design. An automated test can observe cache hits, but the fundamental guarantee is architectural.

**Verification approach:** During Phase 3 code review, confirm:
1. `ModuleIdentContext` is `#[salsa::interned]` (at `src/simlin-engine/src/db.rs:140`)
2. `model_module_ident_context` is `#[salsa::tracked]`
3. The interned type's content includes only the set of module variable names (not mutable state)

---

## Implementation Decision Notes

1. **AC4.4:** `Project::from` may be `#[cfg(test)]`-gated rather than fully deleted, to preserve the AST interpreter cross-validation path (AC4.6).

2. **AC4.5:** `ModelStage1.errors` may be retained with `#[deprecated]` because `Module::new` (retained for interpreter) reads it.

3. **AC7.1:** Four models (`delay.xmile`, `initial.xmile`, `smooth.xmile`, `wrld3-03.mdl`) have known incremental path failures that must be fixed before the monolithic fallback can be deleted.

4. **AC8:** Cache-hit verification depends on salsa's event logging or `salsa::Id` comparison; executor chooses based on what salsa exposes.
