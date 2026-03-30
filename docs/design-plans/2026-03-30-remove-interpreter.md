# Remove AST Interpreter Design

## Summary

This design removes the AST-walking interpreter (`interpreter.rs`, 3,192 lines) and the monolithic `Project::from(datamodel)` compilation pipeline, committing fully to the VM/incremental-compilation path as the sole simulation engine. The interpreter was originally built as a reference specification to validate VM correctness, but all VM gaps have been closed (EXCEPT with dimension mappings, SUM array reductions, ALLOCATE AVAILABLE all pass through the VM now). The interpreter is used only in tests; no production code depends on it. Its removal eliminates ~3,200 lines of interpreter code, the `testing` Cargo feature, and ~1,000+ lines of shadow compilation infrastructure gated behind `cfg(any(test, feature = "testing"))` in model.rs, project.rs, variable.rs, test_common.rs, and ast/. The migration proceeds in six phases: upgrade stale interpreter-only integration tests, add missing VM test helpers, migrate cross-validation tests, extend db_analysis.rs and migrate LTM tests, remove interpreter from integration test harness, and delete interpreter + testing feature + all gated code.

## Definition of Done

Remove the AST-walking interpreter (`interpreter.rs`) and the monolithic `Project::from(datamodel)` compilation pipeline, committing fully to the VM/incremental-compilation path as the sole simulation engine.

**Success criteria:**
1. `interpreter.rs` deleted; `Simulation` type removed from public API
2. `testing` Cargo feature removed; all `cfg(any(test, feature = "testing"))` gated code in model.rs, project.rs, variable.rs, test_common.rs, ast/ removed
3. All ~31 tests using `Project::from()` migrated: LTM tests to salsa-tracked functions, simulation tests to VM helpers
4. Cross-validation tests triaged per analysis: 8 removed, ~15 migrated to VM, 9 kept with inline expected values
5. Interpreter-only integration tests (`simulates_except`, `simulates_except2`, `simulates_longeqns_mdl`, `simulates_sum_interpreter_only`) upgraded to run both paths or converted to VM-only
6. Missing VM test helpers added (`assert_compile_error`, `assert_unit_error`, scalar assertions)
7. All existing tests pass (cargo test, pre-commit hook green)
8. No production behavior changes

**Out of scope:** VM performance optimizations, new feature work, changes to the salsa compilation pipeline itself

## Acceptance Criteria

### remove-interpreter.AC1: Interpreter and testing feature deleted
- **remove-interpreter.AC1.1 Success:** `src/interpreter.rs` does not exist
- **remove-interpreter.AC1.2 Success:** `Simulation` is not exported from `lib.rs`
- **remove-interpreter.AC1.3 Success:** `testing` feature does not appear in `Cargo.toml`
- **remove-interpreter.AC1.4 Success:** No `cfg(any(test, feature = "testing"))` attributes remain in the codebase
- **remove-interpreter.AC1.5 Success:** `cargo build --workspace` produces no dead-code warnings related to removed code
- **remove-interpreter.AC1.6 Success:** `cargo test -p simlin-engine` passes without `--features testing`

### remove-interpreter.AC2: Monolithic compilation path removed
- **remove-interpreter.AC2.1 Success:** `Project::from(datamodel::Project)` (the `From` impl) does not exist
- **remove-interpreter.AC2.2 Success:** `Project::from_datamodel()` does not exist
- **remove-interpreter.AC2.3 Success:** `run_default_model_checks()` in project.rs does not exist
- **remove-interpreter.AC2.4 Success:** Dependency analysis functions gated behind testing in model.rs (`all_deps`, `direct_deps`, `module_deps`, `module_output_deps`, `set_dependencies`, `check_units`) are removed
- **remove-interpreter.AC2.5 Success:** Gated functions in variable.rs (`init_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_only_referenced_idents_with_module_inputs`) are removed
- **remove-interpreter.AC2.6 Success:** `from_salsa()` remains available (ungated) for any test that needs a compiled `Project`

### remove-interpreter.AC3: Integration tests migrated
- **remove-interpreter.AC3.1 Success:** `simulate_path_with()` no longer creates a `Simulation` or calls interpreter `run_to_end()`
- **remove-interpreter.AC3.2 Success:** `simulate_path_interpreter_only()` and `simulate_mdl_path_interpreter_only()` are removed
- **remove-interpreter.AC3.3 Success:** `simulates_except`, `simulates_except2` use `simulate_mdl_path()` (both VM and reference data validation)
- **remove-interpreter.AC3.4 Success:** `simulates_longeqns_mdl` uses `simulate_mdl_path()` (both VM and reference data validation)
- **remove-interpreter.AC3.5 Success:** `simulates_sum_interpreter_only` is removed (redundant with `simulates_sum` which already tests both paths)
- **remove-interpreter.AC3.6 Edge:** `simulates_except_xmile_interpreter_only` (currently `#[ignore]`) is removed or updated

### remove-interpreter.AC4: Cross-validation tests triaged
- **remove-interpreter.AC4.1 Success:** 8 redundant cross-validation tests removed from vm.rs (test_per_var_initials_matches_interpreter, test_per_var_initials_dependency_order, test_per_var_initials_with_module, test_assign_const_curr_simulation_result, test_binop_assign_next_simulation_stock_integration, test_superinstruction_population_model_matches_interpreter, test_superinstruction_with_small_dt)
- **remove-interpreter.AC4.2 Success:** ~15 tests migrated from `run_interpreter()` to `run_vm()` / `run_vm_incremental()` with existing assertions preserved
- **remove-interpreter.AC4.3 Success:** 9 unique-edge-case tests (PREVIOUS chains, SELF refs, cycle breaking, non-divisible save step, unit-mismatch-allows-simulation, implicit subscript through mapped parent, nested INIT) use VM with inline expected values
- **remove-interpreter.AC4.4 Success:** No test in the codebase calls `run_interpreter()`, `build_sim()`, `assert_interpreter_result()`, or `assert_scalar_result()` (the gated interpreter helpers)

### remove-interpreter.AC5: VM test helpers complete
- **remove-interpreter.AC5.1 Success:** `TestProject::assert_compile_error_vm(ErrorCode)` exists and is ungated -- asserts incremental compilation produces the expected error
- **remove-interpreter.AC5.2 Success:** `TestProject::assert_unit_error_vm()` exists and is ungated -- asserts unit checking diagnostics are emitted
- **remove-interpreter.AC5.3 Success:** `TestProject::assert_vm_scalar_result(var_name, expected)` exists -- checks final-timestep value from VM
- **remove-interpreter.AC5.4 Success:** All callers of the removed gated helpers have been migrated to the new ungated VM helpers

### remove-interpreter.AC6: LTM tests migrated to salsa-tracked functions
- **remove-interpreter.AC6.1 Success:** LTM tests in ltm.rs use `model_detected_loops()`, `model_cycle_partitions()`, `model_loop_circuits()` from db_analysis.rs instead of `detect_loops()` / `CausalGraph::from_model()` via `Project::from()`
- **remove-interpreter.AC6.2 Success:** Per-link polarity tests use an exposed `compute_link_polarities()` function (or salsa-tracked equivalent)
- **remove-interpreter.AC6.3 Success:** Tests construct `SimlinDb` + `sync_from_datamodel()` following the pattern established in `db_ltm_unified_tests.rs`
- **remove-interpreter.AC6.4 Edge:** Module loop tests that require recursive module graph analysis work via per-module calls to `model_detected_loops()` or an extended variant

### remove-interpreter.AC7: No production behavior change
- **remove-interpreter.AC7.1 Success:** `cargo test --workspace` passes (all Rust tests)
- **remove-interpreter.AC7.2 Success:** Pre-commit hook passes (Rust + TypeScript + Python)
- **remove-interpreter.AC7.3 Success:** `libsimlin` FFI surface unchanged (no new or removed functions)
- **remove-interpreter.AC7.4 Success:** WASM build succeeds
- **remove-interpreter.AC7.5 Failure:** Removing interpreter causes any change to simulation results for any model

## Glossary

- **AST interpreter**: The tree-walking simulation engine in `interpreter.rs` that evaluates compiled `Expr` AST nodes directly, without compiling to bytecode. Used only in tests for cross-validation against the VM.
- **VM (bytecode VM)**: The stack-based compiled execution engine in `vm.rs` that runs simulations from bytecode produced by the compiler. The production simulation path.
- **Monolithic compilation path**: The `Project::from(datamodel)` constructor that creates a local salsa DB, syncs the entire project, and builds a `Project` struct in one step. Gated behind the `testing` feature. Contrasted with the incremental salsa path.
- **Incremental compilation path**: The production path via `compile_project_incremental()` that uses salsa-tracked functions for fine-grained caching and invalidation.
- **`testing` Cargo feature**: A feature flag that exposes the monolithic `Project::from` construction path and associated test helpers. Not used in production builds.
- **`from_salsa()`**: The ungated lower-level `Project` constructor that takes a pre-synced salsa DB. Used internally by both the monolithic path and the incremental path. Survives this removal.
- **Cross-validation test**: A test that runs both interpreter and VM on the same model and asserts their results match. These lose their purpose when the interpreter is removed.
- **Golden data**: Known-good simulation output (`.dat` or `.csv` files) from other SD software (primarily Vensim) used as reference truth for integration tests.
- **EXCEPT semantics**: Vensim's array equation override syntax (`g[DimA] :EXCEPT: [A1] = 7`) where a default equation applies to most elements and specific elements get overrides.
- **Salsa**: An incremental computation framework. Tracked functions are memoized and recomputed only when their inputs change.
- **Cycle partition**: A group of stocks connected by feedback paths (a strongly connected component in the stock-to-stock reachability graph).

## Architecture

### Current State: Two Parallel Simulation Paths

The engine currently maintains two complete simulation paths:

1. **AST interpreter** (`interpreter.rs`): Tree-walking evaluator that operates on compiled `Expr` AST nodes. Requires the monolithic `Project::from(datamodel)` pipeline which builds `ModelStage0` -> `ModelStage1` -> `ModuleStage2` structures with synchronous dependency resolution, unit checking, and error collection.

2. **Bytecode VM** (`vm.rs`): Stack-based compiled engine. Uses `compile_project_incremental()` via salsa-tracked functions for parsing, dependency analysis, compilation, and assembly. Production code exclusively uses this path.

The monolithic compilation pipeline (`Project::from`) exists solely to feed the interpreter. It duplicates dependency analysis (`all_deps`, `direct_deps`, `module_deps` in model.rs), unit checking (`check_units`), and error collection that the salsa path handles incrementally.

### Target State: Single VM Path

After removal:
- **Simulation**: All simulation goes through `compile_project_incremental()` -> `Vm::new()` -> `run_to_end()`
- **Structural analysis** (LTM, causal graphs): Uses salsa-tracked functions (`model_detected_loops`, `model_causal_edges`, `model_cycle_partitions`) instead of `CausalGraph::from_model()` via `Project::from()`
- **Test infrastructure**: `TestProject` exposes only VM-based helpers (already exist: `run_vm()`, `run_vm_incremental()`, `compile_incremental()`, `assert_vm_result()`)
- **`Project::from_salsa()`** remains ungated for any test that needs a compiled `Project` struct (e.g., for `Module::new()` calls)

### Key Insight: from_salsa() Survives

`Project::from_salsa()` is NOT gated behind `testing` -- it is the internal building block that both the monolithic path and the incremental path use. What gets removed is the convenience wrapper `from_datamodel()` that creates a throwaway local DB. Tests that need a compiled `Project` (rare) can call `from_salsa()` directly with an explicit `SimlinDb`.

### Test Migration Patterns

**Pattern 1: Simulation tests** (most common)
```rust
// Before: run_interpreter() or Simulation::new()
let results = tp.run_interpreter().unwrap();
assert_eq!(results["population"][0], 100.0);

// After: run_vm() (already exists, ungated)
let results = tp.run_vm().unwrap();
assert_eq!(results["population"][0], 100.0);
```

**Pattern 2: Error assertion tests**
```rust
// Before: gated assert_compile_error()
tp.assert_compile_error(ErrorCode::CircularDependency);

// After: new ungated assert_compile_error_vm()
tp.assert_compile_error_vm(ErrorCode::CircularDependency);
```

**Pattern 3: LTM structural analysis tests**
```rust
// Before: Project::from() + detect_loops()
let project = Project::from(datamodel);
let model = &project.models[&Ident::new("main")];
let loops = detect_loops(model, &project).unwrap();

// After: salsa-tracked functions
let db = SimlinDb::default();
let sync = sync_from_datamodel(&db, &datamodel);
let loops = model_detected_loops(&db, sync.models["main"].source, sync.project);
```

**Pattern 4: Integration tests**
```rust
// Before: simulate_path_with() runs both interpreter and VM
let project = Rc::new(Project::from(datamodel_project.clone()));
let sim = Simulation::new(&project, "main").unwrap();
let results1 = sim.run_to_end().unwrap();
// ... then VM ...
let results2 = vm.into_results();
ensure_results(&expected, &results1);
ensure_results(&expected, &results2);

// After: VM only + reference data
let compiled_sim = compile(&datamodel_project);
let mut vm = Vm::new(compiled_sim).unwrap();
vm.run_to_end().unwrap();
let results = vm.into_results();
ensure_results(&expected, &results);
```

## Existing Patterns

The codebase already has extensive VM test infrastructure that this design builds on:

- **`TestProject::run_vm()`** / **`run_vm_incremental()`**: Ungated methods that compile via salsa and run the VM. Used by ~50+ existing tests.
- **`TestProject::compile_incremental()`**: Compiles via `compile_project_incremental()` and returns `CompiledSimulation`. Used to test compilation without running.
- **`db_ltm_unified_tests.rs`**: Established pattern for LTM testing via salsa -- creates `SimlinDb`, syncs datamodel, calls tracked functions. This is the template for LTM test migration.
- **`compile_vm()` in simulate.rs**: Helper that creates a SimlinDb, syncs, and calls `compile_project_incremental()`. Used by integration tests alongside the interpreter leg that we're removing.

## Implementation Phases

### Phase 1: Upgrade interpreter-only integration tests to VM

The four interpreter-only integration tests (`simulates_except`, `simulates_except2`, `simulates_longeqns_mdl`, `simulates_sum_interpreter_only`) are marked interpreter-only due to stale comments about missing VM support. Investigation confirmed all four models pass through the VM with zero relative error.

- Change `simulates_except` and `simulates_except2` from `simulate_mdl_path_interpreter_only()` to `simulate_mdl_path()`
- Change `simulates_longeqns_mdl` from `simulate_mdl_path_interpreter_only()` to `simulate_mdl_path()`
- Remove `simulates_sum_interpreter_only` (redundant with `simulates_sum` which already tests both paths)
- Remove or update `simulates_except_xmile_interpreter_only` (currently `#[ignore]`)
- Update stale comments about VM lacking SUM, ALLOCATE AVAILABLE, etc.

**Verification:** `cargo test --features file_io,testing --test simulate`

### Phase 2: Add missing VM test helpers

Add ungated test helper methods to `TestProject` that parallel the gated interpreter helpers:

- `assert_compile_error_vm(ErrorCode)`: Asserts `compile_incremental()` returns the expected error code
- `assert_unit_error_vm()`: Asserts incremental compilation emits unit-mismatch diagnostics
- `assert_vm_scalar_result(var_name, expected)`: Checks the final-timestep value from VM

These go in the ungated impl block of test_common.rs alongside the existing VM helpers.

**Verification:** New helpers compile and existing tests still pass

### Phase 3: Migrate cross-validation and interpreter-only unit tests

Triage the ~32 tests that call `run_interpreter()` across vm.rs, db_tests.rs, model.rs, unit_checking_test.rs, builtins_visitor.rs, db_fragment_cache_tests.rs, and compiler/dimensions.rs:

**Remove** (8 tests in vm.rs): Pure cross-validation of basic features exhaustively covered by simulate.rs integration tests:
- `test_per_var_initials_matches_interpreter`
- `test_per_var_initials_dependency_order`
- `test_per_var_initials_with_module`
- `test_assign_const_curr_simulation_result`
- `test_binop_assign_next_simulation_stock_integration`
- `test_superinstruction_population_model_matches_interpreter`
- `test_superinstruction_with_small_dt`

Note: `test_fused_binop_next_sub` also drops its interpreter comparison but retains its hardcoded assertions and bytecode shape checks.

**Migrate to VM** (~15 tests): Replace `run_interpreter()` with `run_vm()` or `run_vm_incremental()`, preserving all existing assertions:
- vm.rs: `test_per_var_initials_with_array`, `test_previous_in_initials_vm_matches_interpreter`, `test_fused_binop_next_sub`, `test_multiple_superinstructions_in_one_model`
- model.rs: `test_init_expression_interpreter_vm_parity`
- db_tests.rs: `test_previous_opcode_interpreter_vm_parity`, `test_init_opcode_interpreter_vm_parity`, `test_previous_of_flow_interpreter_vm_parity`, `test_arrayed_1arg_previous_loadprev_per_element`, `test_arrayed_2arg_previous_per_element`
- unit_checking_test.rs: `test_previous_basic_functionality`, `test_previous_with_constant`, `test_previous_with_expression`
- compiler/dimensions.rs: `test_cross_dimension_mapping_simple`, `test_cross_dimension_mapping_reverse`
- builtins_visitor.rs: `test_npv_basic`, `test_npv_with_discount`, `test_modulo_function`

**Keep with inline values** (9 tests): Switch to VM, add hardcoded expected values for unique edge cases not covered by integration tests:
- vm.rs: `test_non_divisible_save_step_interpreter_agreement`
- unit_checking_test.rs: `test_previous_with_self`, `test_previous_with_different_dt_and_save_step`, `test_previous_chain`, `test_unit_mismatch_allows_simulation`, `test_unit_mismatch_in_stock_allows_simulation`
- db_fragment_cache_tests.rs: `test_previous_lagged_feedback_interpreter_path_is_acyclic`
- compiler/dimensions.rs: `test_implicit_subscript_through_mapped_parent_dimension`
- builtins_visitor.rs: `test_nested_init_does_not_rewrite_generated_arg_helpers`

**Verification:** `cargo test -p simlin-engine`

### Phase 4: Extend db_analysis.rs and migrate LTM tests

The ~18 LTM tests in ltm.rs use `Project::from()` to build causal graphs and detect loops. Migrate them to salsa-tracked functions.

**Extend db_analysis.rs:**
- Expose `compute_link_polarities()` as a public (or salsa-tracked) function for per-link polarity tests
- For module loop tests that need recursive module graph analysis, either call `model_detected_loops()` per sub-model or extend the function to handle modules

**Migrate tests** following the `db_ltm_unified_tests.rs` pattern:
- Create `SimlinDb::default()` + `sync_from_datamodel()`
- Call `model_detected_loops()`, `model_cycle_partitions()`, `model_loop_circuits()` instead of `detect_loops()` / `CausalGraph::from_model()`
- Adapt assertions for the salsa return types (e.g., `DetectedLoop.variables` instead of `Loop.links`)

**Verification:** `cargo test -p simlin-engine` (all LTM tests pass)

### Phase 5: Remove interpreter from integration test harness

Remove the interpreter leg from the integration test framework:

- In `simulate_path_with()`: Remove `Project::from` + `Simulation::new` + `run_to_end()` leg. Keep VM compile + run + reference data comparison + protobuf roundtrip + XMILE roundtrip.
- In `simulate_mdl_path()`: Same treatment -- remove interpreter leg
- In `simulate_mdl_path_with_data()`: Same treatment
- Remove `simulate_path_interpreter_only()` and `simulate_mdl_path_interpreter_only()` (no callers remain after Phase 1)
- Remove `use simlin_engine::interpreter::Simulation` imports from test files (simulate.rs, simulate_systems.rs)
- Remove `use simlin_engine::Project` imports that are only used for interpreter construction

**Verification:** `cargo test --features file_io,testing --test simulate --test simulate_systems --test simulate_ltm`

### Phase 6: Delete interpreter, testing feature, and all gated code

The final cleanup:

- Delete `src/interpreter.rs`
- Remove `pub mod interpreter` and `pub use self::interpreter::Simulation` from `lib.rs`
- Remove `testing = []` from Cargo.toml; remove `required-features = ["...", "testing"]` from all `[[test]]` entries (replace with just `file_io` where needed)
- Remove all `#[cfg(any(test, feature = "testing"))]` gated code:
  - project.rs: `From<datamodel::Project>` impl, `from_datamodel()`, `run_default_model_checks()`
  - model.rs: `dt_deps`, `initial_deps`, `init_referenced_vars`, `module_deps`, `module_output_deps`, `direct_deps`, `DepContext`, `all_deps`, `resolve_relative2`, `set_dependencies`, `check_units`
  - variable.rs: `init_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_only_referenced_idents_with_module_inputs`
  - test_common.rs: entire gated impl block (`compile`, `build_sim`, `run_interpreter`, `assert_compile_error`, `assert_unit_error`, `assert_scalar_result`, `assert_interpreter_result`, `interpreter_result`, `flow_runlist_has_assign_temp`, `assert_sim_builds`, `build_module`)
  - ast/expr2.rs: gated `get_var_loc()` methods
  - ast/mod.rs: gated `get_var_loc()` method
- Remove `src/alloc.rs` if it becomes unused (check if VM uses it directly)
- Clean up any remaining dead imports, unused variables

**Verification:** `cargo test --workspace` + full pre-commit hook

## Additional Considerations

### get_var_loc() in db.rs

The `get_var_loc()` methods on `Expr2` and `IndexExpr2` are gated behind `testing` but called from `db.rs` for error diagnostic location reporting. This call is in error-path code that gracefully degrades (uses a default empty location) when the method is unavailable. After removing the `testing` gate, either:
- Un-gate `get_var_loc()` (move it out of the gated block) if diagnostic location accuracy matters
- Remove the call in db.rs and always use the default location

The recommendation is to un-gate `get_var_loc()` since it's a pure read-only method with no dependency on the interpreter.

### alloc.rs shared code

`alloc.rs` contains allocation helpers (`allocate_available()`, `alloc_curve()`, etc.) shared by both interpreter and VM. After interpreter removal, check whether these functions are still referenced by `vm.rs`. If so, they survive unchanged. If any functions become dead code, remove them.

### Stale comments

Many comments reference the interpreter or cross-validation that will need updating:
- CLAUDE.md line 25: "Retained as a reference spec for VM correctness verification"
- simulate.rs: comments about "interpreter-only" tests, "VM lacks X"
- project.rs: "Retained only for tests and the AST interpreter cross-validation path"
- test_common.rs: comments about monolithic path

Update or remove all stale comments as part of Phase 6.

### CLAUDE.md updates

After removal, update `src/simlin-engine/CLAUDE.md`:
- Remove interpreter.rs from compilation pipeline documentation
- Remove `testing` feature from Cargo features section
- Update test documentation to reflect VM-only testing
- Remove references to cross-validation between interpreter and VM
