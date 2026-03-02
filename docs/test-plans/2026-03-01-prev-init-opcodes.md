# Test Plan: Activate PREVIOUS/INIT Opcodes

## Prerequisites

- Rust toolchain installed (edition 2021+)
- Working directory: `/home/bpowers/src/simlin`
- All automated tests passing: `cargo test -p simlin-engine`
- Git history available for structural verification

## Phase 1: Structural Verification of AC4.2 -- PREVIOUS Module Handling in LTM

Purpose: AC4.2 (PREVIOUS-specific module handling removed from LTM) was SKIPPED and tracked as GitHub #370 due to PREVIOUS(TIME) dependency. Verify the current state.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open `src/simlin-engine/src/db_ltm.rs` | File exists and is readable |
| 2 | Search for `implicit_module_vars` declarations | Variable named `implicit_module_vars` still exists (PREVIOUS(TIME) still needs module expansion) |
| 3 | Search for `ltm_implicit_info` fetch or usage | Still present (PREVIOUS(TIME) cross-LTM deps) |
| 4 | Verify SMOOTH/DELAY module handling is intact: search for `implicit_info`, `dep_variables`, `implicit_module_refs`, `implicit_submodels` | All present and functional |
| 5 | Run `cargo test -p simlin-engine --test simulate_ltm -- test_smooth_with_initial_value_ltm test_smooth_goal_seeking_ltm test_multiple_smooth_instances` | All 3 SMOOTH tests pass |

## Phase 2: Structural Verification of AC5.1 -- stdlib/init.stmx Deletion

Purpose: Confirm the init stdlib model file was deleted from the repository.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `ls stdlib/init.stmx` | File should not exist |
| 2 | Run `git log --all --diff-filter=D -- stdlib/init.stmx` | Commit shows file was deleted |
| 3 | Search `src/simlin-engine/src/stdlib.gen.rs` for `fn init()` | No `init()` function exists |
| 4 | List `stdlib/` directory | 7 models remain (delay1, delay3, npv, previous, smth1, smth3, trend). No `init.stmx`. |

## Phase 3: Comment Hygiene Verification of AC6.2

Purpose: Confirm scaffolding/deferred-activation comments were updated.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Search `src/simlin-engine/src/builtins_visitor.rs` for "scaffolding", "deferred", "Task 4", "until then" | None of these phrases appear |
| 2 | Search `src/simlin-engine/src/compiler/codegen.rs` near unreachable guards for same phrases | No scaffolding language |
| 3 | Search `src/simlin-engine/src/bytecode.rs` LoadPrev/LoadInitial doc comments | Accurate descriptions without "placeholder" or "future" language |

## Phase 4: Full Regression

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo test -p simlin-engine` | All tests pass |
| 2 | `cargo test -p simlin-engine --test simulate` | All integration tests pass |
| 3 | `cargo test -p simlin-engine --test simulate_ltm` | All LTM tests pass |
| 4 | `cargo test -p simlin-engine --lib -- test_arrayed` | Both arrayed PREVIOUS tests pass |

## Phase 5: Arrayed PREVIOUS Consistency

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `test_arrayed_1arg_previous_loadprev_per_element` | t=0: both elements return 0.0 (LoadPrev zeros). t=1+: a1=10, a2=20 |
| 2 | Run `test_arrayed_2arg_previous_per_element_modules` | t=0: both elements return 99.0 (init_val from module). t=1+: a1=10, a2=20 |
| 3 | Compare t=1+ values between tests | Identical per-element values, confirming only t=0 differs |

## Traceability

| AC | Automated Test | Manual Step |
|----|----------------|-------------|
| AC1.1 Scalar LoadPrev | `test_previous_opcode_interpreter_vm_parity` | -- |
| AC1.2 Arrayed LoadPrev | `test_arrayed_1arg_previous_loadprev_per_element` | -- |
| AC1.3 PREVIOUS=0 at t=0 | `test_previous_returns_zero_at_first_timestep` | -- |
| AC2.1 LoadInitial | `test_init_opcode_interpreter_vm_parity` | -- |
| AC2.2 INIT aux-only | `test_init_aux_only_model`, `simulates_init_builtin` | -- |
| AC2.3 initial_values populated | `test_init_aux_only_model` (implicit) | -- |
| AC3.1 2-arg module expansion | `test_2arg_previous_uses_module_expansion`, `simulates_previous` | -- |
| AC3.2 Arrayed 2-arg modules | `test_arrayed_2arg_previous_per_element_modules` | -- |
| AC4.1 LTM LoadPrev | `simulates_population_ltm`, discovery tests | -- |
| AC4.2 LTM PREVIOUS removal | SKIPPED (GitHub #370) | Phase 1 |
| AC5.1 init.stmx deleted | `test_previous_still_in_stdlib_model_names`, model count | Phase 2 |
| AC6.1 All tests pass | Full test suites | Phase 4 |
| AC6.2 Comments updated | -- | Phase 3 |
