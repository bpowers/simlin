# Test Requirements: Salsa Consolidation

**Document purpose:** Map every acceptance criterion from the salsa-consolidation design to either an automated test or a documented human verification.

**Conventions:**
- File paths beginning with `src/simlin-engine/` are in the `simlin-engine` crate.
- File paths beginning with `src/libsimlin/` are in the `libsimlin` crate.
- All Rust test runs assume `--features file_io` unless noted.
- "Existing test" means the test pre-exists and must continue to pass unchanged.
- "New test" means written as part of the implementation phase.

---

## AC1: LTM Fully Incrementalized

| AC | Test Type | Test File | Test Name | Notes |
|----|-----------|-----------|-----------|-------|
| AC1.1 | Integration | `tests/simulate_ltm.rs` | `simulate_ltm_path` (existing) | Must pass unchanged after Phase 2 removes monolithic fallback |
| AC1.1 | Unit | `src/db_tests.rs` | `test_ltm_incremental_matches_monolithic_all_models` (new) | Cross-validates incremental LTM against reference TSV |
| AC1.2 | Unit (salsa events) | `src/db_tests.rs` | `test_ltm_no_recompile_on_unchanged_dependency_edit` (new) | Verify DidExecuteFunction NOT fired for compile_ltm_var_fragment |
| AC1.3 | Unit (salsa events) | `src/db_tests.rs` | `test_ltm_partial_recompile_on_changed_dependency` (new) | Two independent loops, verify only affected loop recompiles |
| AC1.4 | Unit | `src/db_tests.rs` | `test_ltm_no_overhead_without_feedback_loops` (new) | Acyclic model with ltm_enabled=true has zero LTM slots |
| AC1.5 | Unit | `src/db_tests.rs` | `test_ltm_disabled_produces_identical_bytecode` (new) | Byte-level equality of compiled bytecode |
| AC1.6 | Unit | `src/db_tests.rs` | `test_ltm_discovery_mode_all_links_get_score_variables` (new) | ltm_discovery_mode=true returns scores for ALL causal links |
| AC1.7 | Unit (salsa events) | `src/db_tests.rs` | `test_ltm_stdlib_module_composite_scores_cached` (new) | SMOOTH module ilink fragments not recomputed on unrelated edits |

## AC2: Patch Error Checking Uses Incremental Path Only

| AC | Test Type | Test File | Test Name | Notes |
|----|-----------|-----------|-----------|-------|
| AC2.1 | Integration (FFI) | `src/tests_incremental.rs` | All existing AC3.x tests | Must pass unchanged after Phase 4 |
| AC2.2 | Unit + Integration | `src/db_tests.rs`, `src/tests_incremental.rs` | `test_error_bad_table_specific_code`, `test_patch_bad_table_error_code` (new) | Assert ErrorCode::BadTable, not NotSimulatable |
| AC2.3 | Unit + Integration | `src/db_tests.rs`, `src/tests_incremental.rs` | `test_error_empty_equation_specific_code`, `test_patch_empty_equation_error_code` (new) | Stock with no equation |
| AC2.4 | Unit + Integration | `src/db_tests.rs`, `src/tests_incremental.rs` | `test_error_mismatched_dimensions_specific_code`, `test_patch_mismatched_dimensions_error_code` (new) | Array dimension mismatch |
| AC2.5 | Unit + Integration | `src/db_tests.rs`, `src/tests_incremental.rs` | `test_unit_warning_accumulated_with_warning_severity`, `test_patch_unit_warning_causes_rejection` (new) | Severity::Warning for unit errors |
| AC2.6 | Structural | N/A | N/A | Deletion of compile_simulation enforced by cargo build |
| AC2.7 | Unit + HV | `src/db_tests.rs` | `test_vm_validation_error_accumulated_and_causes_rejection` (new) | See HV-3 below |

## AC3: Old Bytecode Compilation Path Deleted

| AC | Test Type | Test File | Notes |
|----|-----------|-----------|-------|
| AC3.1 | Structural | N/A | cargo build after deletion; grep verification |
| AC3.2 | Structural + HV | N/A | Simulation::compile and Module::compile deleted; build_metadata RETAINED (see HV-1) |
| AC3.3 | Structural | N/A | compile_simulation deleted; grep verification |
| AC3.4 | Integration | `tests/simulate.rs`, `tests/simulate_ltm.rs`, `benches/` | Existing tests migrated to incremental path |
| AC3.5 | Structural + HV | N/A | ModelStage1.errors removed; Variable.errors RETAINED (see HV-2) |

## AC4: Interpreter Preserved

| AC | Test Type | Test File | Test Name | Notes |
|----|-----------|-----------|-----------|-------|
| AC4.1 | Integration | `tests/simulate.rs` | `simulate_path` interpreter leg (existing) | Unaffected by VM path migration |
| AC4.2 | Unit | `src/db_tests.rs` | `test_module_new_produces_expr_runlists` (new) | Verify Module::new still works after deletions |
| AC4.3 | Integration | `tests/simulate.rs` | `simulate_path` cross-validation (existing) | Interpreter vs VM for all models |

## AC5: All Existing Tests Pass

| AC | Test Type | Test File | Notes |
|----|-----------|-----------|-------|
| AC5.1 | Integration | `tests/simulate.rs` | `cargo test -p simlin-engine --features file_io -- simulate` |
| AC5.2 | Integration | `tests/simulate_ltm.rs` | `cargo test -p simlin-engine --features file_io -- simulate_ltm` |
| AC5.3 | Integration (FFI) | `src/tests_incremental.rs`, `src/tests_remaining.rs` | `cargo test -p libsimlin` |
| AC5.4 | Integration + Unit | `tests/simulate.rs`, `src/db_tests.rs` | `test_previous_init_numerical_equivalence_interpreter_vs_vm` (new) |

## AC6: PREVIOUS and INIT as Builtins

| AC | Test Type | Test File | Test Name | Notes |
|----|-----------|-----------|-----------|-------|
| AC6.1 | Unit (bytecode) | `src/db_tests.rs` | `test_previous_compiles_to_load_prev_opcode` (new) | Assert LoadPrev present, no EvalModule |
| AC6.2 | Unit (bytecode) | `src/db_tests.rs` | `test_init_compiles_to_load_initial_opcode` (new) | Assert LoadInitial present, value constant across timesteps |
| AC6.3 | Unit (bytecode) + HV | `src/db_tests.rs` | `test_nested_previous_compiles_to_two_load_prev_opcodes` (new) | See HV-4 below |
| AC6.4 | Unit (static) | `src/db_tests.rs` | `test_previous_and_init_not_in_stdlib_model_names` (new) | Assert not in MODEL_NAMES |

---

## Human Verification Items

### HV-1: AC3.2 -- build_metadata retention

**Issue:** AC text says "build_metadata no longer exists." Implementation plan says RETAIN it (Module::new dependency).
**Action:** Reviewer confirms build_metadata is intentionally retained and the AC text is erroneous. Update AC text or add explanatory comment.

### HV-2: AC3.5 -- Variable.errors retention

**Issue:** AC text says "No struct-field error walking remains." Implementation plan retains Variable.errors/unit_errors (lowering pipeline dependency).
**Action:** Reviewer confirms retained fields are NOT read by any production error collection path after Phase 6. Fields remain as write-only artifacts.

### HV-3: AC2.7 -- Vm::new validation error coverage

**Issue:** Compiler normally produces valid bytecode; constructing failing test is difficult.
**Action:** Code review confirms Vm::new is called in apply_patch flow and Err result is handled. Existing Vm::new unit tests cover the validator's own logic.

### HV-4: AC6.3 -- Nested PREVIOUS(PREVIOUS(x)) semantics

**Issue:** Two LoadPrev opcodes for same variable read same curr[off] value.
**Action:** During Phase 1, implementer checks link_score_equation_text for nested PREVIOUS usage and verifies whether identical values are the intended LTM semantics.
