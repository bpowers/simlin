# Test Plan: Unify PREVIOUS/INIT Dependency Extraction

## Prerequisites

- Development environment initialized: `./scripts/dev-init.sh`
- All automated tests passing: `cargo test -p simlin-engine --features file_io`
- Branch `unify-dep-extraction` checked out

## Phase 1: Verify Thin Wrapper Conversion (AC1.5)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open `src/simlin-engine/src/variable.rs` | File opens |
| 2 | Search for `struct IdentifierSetVisitor` | No results -- struct and impl block deleted |
| 3 | Search for `fn identifier_set(`. Read function body | Single expression: `classify_dependencies(ast, dimensions, module_inputs).all` |
| 4 | Search for `fn init_referenced_idents(`. Read body | `classify_dependencies(ast, &[], None).init_referenced` |
| 5 | Search for `fn previous_referenced_idents(`. Read body | `classify_dependencies(ast, &[], None).previous_referenced` |
| 6 | Search for `fn lagged_only_previous_idents_with_module_inputs(`. Read body | `classify_dependencies(ast, &[], module_inputs).previous_only` |
| 7 | Search for `fn init_only_referenced_idents_with_module_inputs(`. Read body | `classify_dependencies(ast, &[], module_inputs).init_only` |
| 8 | Confirm all 5 wrappers retain original `pub` visibility, signatures, and doc comments | Signatures unchanged; only bodies replaced |

## Phase 2: Verify db.rs Call Consolidation (AC2.1, AC2.2)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open `src/simlin-engine/src/db.rs`. Find `fn variable_direct_dependencies_impl` | Function found |
| 2 | In the non-Module arm, count calls to `classify_dependencies` | Exactly 2: one for dt AST, one for init AST |
| 3 | Search same function for old wrapper function calls | Zero matches |
| 4 | Verify `VariableDeps` fields populated from `DepClassification` fields directly | Direct field mapping confirmed |
| 5 | Open `src/simlin-engine/src/db_implicit_deps.rs`. Find `fn extract_implicit_var_deps` | Function found |
| 6 | Count calls to `classify_dependencies` in the closure | Exactly 2 |
| 7 | Search same function for old wrapper function calls | Zero matches |

## Phase 3: Verify Shared Module Predicate (AC3.1, AC3.2)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open `src/simlin-engine/src/builtins.rs`. Find `fn is_stdlib_module_function` | Exists as `pub(crate)` |
| 2 | Examine function body | Checks `MODEL_NAMES` plus `"delay" / "delayn" / "smthn"` |
| 3 | Open `src/simlin-engine/src/model.rs`. Find `equation_is_stdlib_call` | Calls `crate::builtins::is_stdlib_module_function(...)` |
| 4 | Open `src/simlin-engine/src/builtins_visitor.rs`. Find `contains_stdlib_call` | Calls `crate::builtins::is_stdlib_module_function(...)` |
| 5 | `grep -rn '"delayn"' src/simlin-engine/src/` (exclude tests) | Single definition in `is_stdlib_module_function` |

## Phase 4: Verify Matrix Test Completeness (AC4.1)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Navigate to `test_classify_dependencies_matrix` in variable.rs | Test found |
| 2 | Map all case labels to (reference form, context) pairs | All 20 core cells of the 5x4 matrix covered |
| 3 | Count total test cases | 29 (20 core + 9 edge cases/variants) |
| 4 | `cargo test -p simlin-engine test_classify_dependencies_matrix -- --nocapture` | All cases pass |

## Phase 5: Verify Differential Tests (AC5.1, AC5.2, AC5.3)

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo test -p simlin-engine --features file_io test_fragment_phase_agreement_integration` | 18 models checked, test passes |
| 2 | `cargo test -p simlin-engine test_fragment_phase_agreement_synthetic` | All 5 synthetic tests pass |
| 3 | Read `assert_fragment_phase_agreement` in `db_differential_tests.rs` | Dynamic iteration over all variables, not hardcoded |
| 4 | Verify 5 synthetic models match requirements | PREVIOUS feedback, INIT-only, nested, SMOOTH, mixed |

## End-to-End: Full Regression

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo test -p simlin-engine --features file_io --test simulate` | All simulation tests pass |
| 2 | `cargo test -p simlin-engine --features file_io --test roundtrip` | Roundtrip tests pass |
| 3 | `cargo test -p simlin-engine --features file_io --test json_roundtrip` | JSON roundtrip tests pass |
| 4 | `cargo test -p simlin-engine` | All unit tests pass |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC0.1 | `tests/simulate.rs` | End-to-End step 1 |
| AC0.2 | `cargo test -p simlin-engine` | End-to-End step 4 |
| AC0.3 | full integration suite | End-to-End steps 1-3 |
| AC1.1-AC1.4, AC1.6 | `test_classify_dependencies_matrix` | -- |
| AC1.5 | -- | Phase 1 steps 2-8 |
| AC2.1 | -- | Phase 2 steps 1-4 |
| AC2.2 | -- | Phase 2 steps 5-7 |
| AC2.3 | `tests/simulate.rs` | End-to-End step 1 |
| AC3.1 | -- | Phase 3 steps 1-4 |
| AC3.2 | -- | Phase 3 step 5 |
| AC4.1-AC4.3 | `test_classify_dependencies_matrix` | Phase 4 steps 1-4 |
| AC5.1 | `test_fragment_phase_agreement_integration_models` | Phase 5 step 1 |
| AC5.2 | `test_fragment_phase_agreement_synthetic_*` | Phase 5 step 2 |
| AC5.3 | structural property of `assert_fragment_phase_agreement` | Phase 5 steps 3-4 |
