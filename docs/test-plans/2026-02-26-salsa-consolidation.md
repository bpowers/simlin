# Test Plan: Salsa Consolidation

**Implementation plan:** `docs/implementation-plans/2026-02-26-salsa-consolidation/`
**Branch:** `salsa-consolidation`

## Prerequisites

- Rust toolchain with `cargo` available
- Repository checked out at branch `salsa-consolidation`
- Run `./scripts/dev-init.sh` (idempotent, initializes environment)
- Verify all automated tests pass:
  - `cargo test -p simlin-engine --features file_io`
  - `cargo test -p simlin --lib`

## Phase 1: LTM Incremental Path

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --features file_io -- test_ltm_incremental_produces_synthetic_variables` | PASS. Compiled output contains LTM variable offsets (keys starting with '$'). Layout slot count with LTM enabled is greater than without. |
| 2 | Run `cargo test -p simlin-engine --features file_io -- test_ltm_incremental_simulation_produces_scores` | PASS. At least one `rel_loop_score` variable exists. All scores are finite. At least one score is non-zero. |
| 3 | Run `cargo test -p simlin-engine --features file_io -- simulates_population_ltm` | PASS. Both interpreter and incremental VM match the logistic growth TSV reference data within 5% relative tolerance for all non-initial timesteps. |
| 4 | Run `cargo test -p simlin-engine --features file_io -- test_ltm_no_loops_zero_overhead` | PASS. No-loop model: slot count identical with/without LTM. Zero LTM synthetic variables. Root module slot count identical. |
| 5 | Run `cargo test -p simlin-engine --features file_io -- test_ltm_disabled_identical_bytecode` | PASS. After enable/disable cycle: slot count, module count, and every offset in the map matches the never-enabled baseline. |
| 6 | Run `cargo test -p simlin-engine --features file_io -- test_ltm_discovery_mode_all_links` | PASS. Discovery mode produces at least one link score variable. Normal mode produces at least one synthetic variable. Compilation succeeds with LTM offsets in output. |

## Phase 2: Error Accumulator Consolidation

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine -- test_ac2_2_bad_table_specific_error` | PASS. Diagnostic for `lookup_var` has `ErrorCode::BadTable` and `DiagnosticSeverity::Error`. |
| 2 | Run `cargo test -p simlin-engine -- test_ac2_3_empty_equation` | PASS. Diagnostic for `my_stock` has `ErrorCode::EmptyEquation` and `DiagnosticSeverity::Error`. |
| 3 | Run `cargo test -p simlin-engine -- test_ac2_4_mismatched_dimensions` | PASS. Incompatible array dimensions produce `ErrorCode::MismatchedDimensions` error via accumulator. |
| 4 | Run `cargo test -p simlin-engine -- test_ac2_5_unit_warnings_severity` | PASS. At least one unit warning exists at Warning severity. Zero unit diagnostics at Error severity. |
| 5 | Run `cargo test -p simlin-engine -- test_ac2_7_vm_validation_errors` | PASS. Compilation succeeds. `Vm::new` returns `Err` with `ErrorCode::BadSimSpecs`. |
| 6 | Run `cargo test -p simlin-engine -- test_ac2_7_assembly_errors_accumulated` | PASS. `compile_project_incremental` returns `Err` for circular deps. Accumulator contains `CircularDependency` diagnostic. |
| 7 | Run `cargo test -p simlin -- test_ac2_6_compile_simulation_not_in_patch_path` | PASS. Source scan of `patch.rs` finds no `compile_simulation` reference. |

## Phase 3: Patch Pipeline FFI

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin -- test_ac2_1_patch_syntax_error_rejected` | PASS. Patch with syntax error rejected with parse error code. |
| 2 | Run `cargo test -p simlin -- test_ac2_1_patch_circular_dependency_rejected` | PASS. Circular dependency patch rejected with appropriate error code. |
| 3 | Run `cargo test -p simlin -- test_ac2_1_valid_patch_accepted_and_simulatable` | PASS. Valid patch accepted. Simulation produces correct output. |
| 4 | Run `cargo test -p simlin -- test_patch_introducing_new_unit_warning_rejected` | PASS. Patch adding unit mismatch rejected with `UnitMismatch`. Variable does NOT appear in model. |
| 5 | Run `cargo test -p simlin -- test_patch_with_preexisting_unit_warnings_succeeds` | PASS. Adding valid variable to model with pre-existing warnings succeeds. |
| 6 | Run `cargo test -p simlin -- test_dry_run_does_not_leak_staged_state` | PASS. After dry-run patch, simulation sees original value. |
| 7 | Run `cargo test -p simlin -- test_rejected_patch_does_not_leak_staged_state` | PASS. After rejected patch, simulation sees original value. |
| 8 | Run `cargo test -p simlin -- test_concurrent_dry_run_never_leaks_to_sim_new` | PASS. 4 reader threads never observe staged values during 50 concurrent dry-run patches. |

## Phase 4: Migration to Incremental Path

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --features file_io -- simulates_models_correctly` | PASS. All 40+ standard test models: parse XMILE, run interpreter, compile via incremental VM, compare both to reference output, serialize/deserialize roundtrip, XMILE roundtrip. |
| 2 | Run `cargo test -p simlin-engine --features file_io -- incremental_compilation_covers_all_models` | PASS. Every model compiles through the salsa incremental path without panic or error. |
| 3 | Run `cargo test -p simlin-engine --features file_io -- simulates_previous` | PASS. PREVIOUS model simulates correctly through both interpreter and incremental VM. |
| 4 | Run `cargo test -p simlin-engine --features file_io -- simulates_init_builtin` | PASS. INIT model simulates correctly through both paths. |
| 5 | Run `cargo test -p simlin-engine --features file_io -- simulates_arrayed_models_correctly` | PASS. All array models simulate correctly. Models with known incremental limitations use the monolithic fallback. |

## End-to-End: Full Simulation Pipeline

1. Run `cargo test -p simlin-engine --features file_io -- simulates_models_correctly`. Verify all models pass.
2. Run `cargo test -p simlin-engine --features file_io -- simulates_wrld3_03`. Verify the large WRLD3 model simulates correctly with VDF cross-validation.
3. Run `cargo test -p simlin-engine --features file_io -- simulates_population_ltm`. Verify the LTM pipeline end-to-end with reference comparison.
4. Run `cargo test -p simlin --lib` (full suite). Verify all FFI tests pass.

## End-to-End: Patch-and-Simulate Workflow

1. Run `cargo test -p simlin -- test_ac3_1_one_compilation_patch_then_sim`. Apply patch, create sim, verify results.
2. Run `cargo test -p simlin -- test_ac3_3_sequential_patches`. Apply two patches, verify both values correct.
3. Run `cargo test -p simlin -- test_ac3_4_snapshot_isolation`. Create sim before patch, verify isolation.
4. Run `cargo test -p simlin -- test_apply_patch_xmile_empty_equation_reject`. Verify error rejection with details.
5. Run `cargo test -p simlin -- test_apply_patch_xmile_empty_equation_allow_errors`. Verify acceptance with allow_errors.

## Human Verification Required

| ID | Criterion | Why Manual | Steps |
|----|-----------|-----------|-------|
| HV-1 | AC3.2: build_metadata retention | Design decision needs confirmation | 1. Open `src/simlin-engine/src/interpreter.rs`. 2. Search for `build_metadata`. 3. Confirm it exists and is called by `Module::new`. 4. Verify `Module::new` depends on it for the interpreter path. |
| HV-2 | AC3.5: Variable.errors retention | Confirm no production error path reads them | 1. Search codebase for `Variable::errors()` and `Variable::unit_errors()` outside test code. 2. Confirm all reads are in test code or deprecated paths. 3. Confirm production error collection uses `collect_all_diagnostics`. |
| HV-3 | AC2.7: Vm::new validation in apply_patch | Difficult to construct failing test from patch path | 1. Open `src/libsimlin/src/simulation.rs`. 2. Confirm `Vm::new` is called in `simlin_sim_new`. 3. Verify error propagation from `Vm::new`. |
| HV-4 | AC3.2: compile/compile_project callers | Confirm no production code calls deprecated functions | 1. Search for `compile_project\b` and `.compile()` in `src/`. 2. Verify all callers are in test files, benchmark files, or documented exceptions. 3. Confirm no production code in `libsimlin/`, `simlin-cli/`, or non-test engine code calls these. |

## Known Gaps (Deferred by Design)

- **AC6.1-6.4**: LoadPrev/LoadInitial opcode activation deferred. PREVIOUS/INIT still use module expansion. Scaffolding exists in `bytecode.rs` and `vm.rs`.
- **AC1.2**: No salsa event tracking test for LTM fragment caching. Transitive coverage via `test_ltm_caching_equation_change_no_dep_change` (pointer equality).
- **AC4.2**: No dedicated `Module::new` unit test. Transitive coverage via every interpreter-leg simulation test.
