# Unify PREVIOUS/INIT Dependency Extraction -- Test Requirements

This document maps every acceptance criterion from the design to either an automated test or a documented human verification step. Each entry references the implementation phase and task that produces the test.

---

## Coverage Summary

| Criterion | Type | Verification Method |
|-----------|------|-------------------|
| AC0.1 | Automated (integration) | Existing `tests/simulate.rs` suite, run at each phase boundary |
| AC0.2 | Automated (unit) | Existing `cargo test -p simlin-engine`, run at each phase boundary |
| AC0.3 | Automated (integration) | Full `cargo test -p simlin-engine --features file_io`, run after each phase |
| AC1.1 | Automated (unit) | Table-driven matrix test in `variable.rs` |
| AC1.2 | Automated (unit) | Table-driven matrix test in `variable.rs` |
| AC1.3 | Automated (unit) | Table-driven matrix test in `variable.rs` |
| AC1.4 | Automated (unit) | Table-driven matrix test in `variable.rs` |
| AC1.5 | Human verification | Code review: wrapper function bodies |
| AC1.6 | Automated (unit) | Table-driven matrix test in `variable.rs` |
| AC2.1 | Human verification | Code review: call count in `variable_direct_dependencies_impl` |
| AC2.2 | Human verification | Code review: call count in `extract_implicit_var_deps` |
| AC2.3 | Automated (integration) | Existing integration tests confirm identical dep graphs |
| AC3.1 | Human verification | Code review: both callers use `is_stdlib_module_function` |
| AC3.2 | Human verification | Code review: no duplicated name-set logic |
| AC4.1 | Automated (unit) | Table-driven matrix test enumerates all combinations |
| AC4.2 | Automated (unit) | Test runner asserts all 5 fields per case |
| AC4.3 | Automated (unit) | Named edge-case entries in matrix |
| AC5.1 | Automated (integration) | Differential check over integration test models |
| AC5.2 | Automated (unit) | Differential check over synthetic models |
| AC5.3 | Automated (unit/integration) | Structural property of the differential test |

---

## AC0: Regression Safety

### AC0.1: All existing simulation tests pass at each phase boundary

- **Test type:** Integration
- **Test file:** `src/simlin-engine/tests/simulate.rs`
- **Verification command:** `cargo test -p simlin-engine --features file_io --test simulate`
- **Trigger:** Run after each phase commit (phases 1-5). The pre-commit hook runs the full test suite, so every commit implicitly checks this.
- **Rationale:** The simulate.rs suite covers 60+ models with known-good outputs. Any change to dependency ordering that affects simulation results will cause a numeric mismatch here.
- **Phase coverage:** All phases (1-5). The implementation plans for every phase list this as a verification step.

### AC0.2: All existing engine unit tests pass at each phase boundary

- **Test type:** Unit
- **Test file:** All unit tests under `src/simlin-engine/src/` (run via `cargo test -p simlin-engine --lib`)
- **Verification command:** `cargo test -p simlin-engine`
- **Trigger:** Run after each phase commit. The pre-commit hook covers this.
- **Rationale:** Unit tests in variable.rs, db.rs, model.rs, builtins_visitor.rs exercise the exact functions being refactored. Compilation failure or assertion failure here catches API mismatches and behavioral regressions at the function level.
- **Phase coverage:** All phases (1-5).

### AC0.3: Full integration suite passes after each phase

- **Test type:** Integration
- **Test file:** `src/simlin-engine/tests/simulate.rs`, `src/simlin-engine/tests/roundtrip.rs`, `src/simlin-engine/tests/json_roundtrip.rs`, `src/simlin-engine/tests/compiler_vector.rs`, and all other integration test files under `src/simlin-engine/tests/`
- **Verification command:** `cargo test -p simlin-engine --features file_io`
- **Trigger:** Run after each phase commit.
- **Rationale:** Beyond simulation correctness (AC0.1), this catches regressions in XMILE roundtripping, MDL equivalence, vector compilation, and layout -- any of which could be disrupted if dependency extraction changes affect compilation paths.
- **Phase coverage:** All phases (1-5).

---

## AC1: Single unified dependency analysis pass

### AC1.1: classify_dependencies() returns correct sets for mixed PREVIOUS/INIT/direct references

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs` (in the `#[cfg(test)]` module)
- **Test name:** `test_classify_dependencies_matrix`
- **Specific cases covering AC1.1:**
  - `mixed_prev_current`: `PREVIOUS(b) + b` -- verifies `all={b}`, `previous_referenced={b}`, `previous_only={}` (b also appears outside PREVIOUS), `init_only={}`, `init_referenced={}`
  - `mixed_init_current`: `INIT(b) + b` -- verifies `init_referenced={b}`, `init_only={}` (b also appears outside INIT)
  - `both_lagged_scalar`: `PREVIOUS(b) + INIT(b)` -- verifies all five fields with b in both lagged sets but not outside either
  - `both_lagged_different`: `PREVIOUS(a) + INIT(b)` -- verifies correct partitioning when PREVIOUS and INIT target different variables
- **Assertion approach:** Each case asserts all 5 fields of `DepClassification` by comparing against expected `HashSet`/`BTreeSet` values. Case labels are included in assertion messages.
- **Implementation phase:** Phase 4, Task 1 (test creation). Phase 1, Task 1 (function under test).

### AC1.2: classify_dependencies() handles ApplyToAll and Arrayed AST variants

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs`
- **Test name:** `test_classify_dependencies_matrix`
- **Specific cases covering AC1.2:**
  - `direct_a2a`: `a + b` wrapped in `Ast::ApplyToAll(dim, expr)` -- confirms walker enters the ApplyToAll expression
  - `direct_arrayed`: `Ast::Arrayed` with element "e1"=`a`, default=`b` -- confirms walker visits all element expressions and the default expression
  - `previous_a2a`: `PREVIOUS(b)` in `Ast::ApplyToAll` -- confirms flag-tracking works through ApplyToAll
  - `init_a2a`: `INIT(b)` in `Ast::ApplyToAll` -- same for INIT
  - `both_lagged_a2a`: `PREVIOUS(b) + INIT(b)` in `Ast::ApplyToAll` -- combined
  - `mixed_prev_current_a2a`: `PREVIOUS(b) + b` in `Ast::ApplyToAll` -- mixed reference in A2A context
- **AST construction:** These cases use direct `Ast::ApplyToAll(...)` / `Ast::Arrayed(...)` construction rather than text parsing, since the parser does not produce these variants from flat equation text.
- **Implementation phase:** Phase 4, Task 1.

### AC1.3: IsModuleInput branch selection produces identical deps to old IdentifierSetVisitor

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs`
- **Test name:** `test_classify_dependencies_matrix`
- **Specific cases covering AC1.3:**
  - `direct_ismoduleinput`: `if isModuleInput(input) then a else b` with `module_inputs={input}` -- only the "then" branch is walked, so `all={a}` (not {a, b, input})
  - `previous_ismoduleinput`: `if isModuleInput(input) then PREVIOUS(a) else b` with `module_inputs={input}` -- PREVIOUS inside pruned branch, verifies flag tracking works with branch pruning
  - `init_ismoduleinput`: `if isModuleInput(input) then INIT(a) else b` with `module_inputs={input}`
  - `mixed_prev_current_ismoduleinput`: `if isModuleInput(input) then PREVIOUS(a) + a else b` with `module_inputs={input}` -- mixed reference in pruned branch
  - `ismoduleinput_else_branch`: same If expression but module_inputs does NOT contain "input", so the "else" branch is selected -- verifies the negation path
  - `ismoduleinput_no_pruning`: same If expression with `module_inputs=None` -- all branches walked, verifies no-pruning fallback
- **Rationale:** The old `IdentifierSetVisitor` had explicit IsModuleInput handling (lines 761-775 of variable.rs). The new `ClassifyVisitor` must reproduce this exactly. The `ismoduleinput_else_branch` and `ismoduleinput_no_pruning` cases specifically test the two code paths that differ from naive "walk all branches."
- **Implementation phase:** Phase 4, Task 1.

### AC1.4: IndexExpr2::Range endpoints are walked and dimension names are filtered

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs`
- **Test name:** `test_classify_dependencies_matrix`
- **Specific cases covering AC1.4:**
  - `direct_range`: `arr[1:CONST]` constructed as `Expr2::Subscript` with `IndexExpr2::Range` -- verifies both endpoints are walked (CONST appears in `all`)
  - `previous_range`: range endpoint containing `PREVIOUS(lagged)` -- verifies flag tracking through range walking
  - `init_range`: range endpoint containing `INIT(seed)` -- same for INIT
  - `dim_filtering`: `a + foo` with `dimensions=[{name: "dim1", elements: ["foo"]}]` -- verifies that `foo` is filtered from `all` because it matches a dimension element
- **AST construction:** Range cases require direct `Expr2` construction since `IndexExpr2::Range` is not produced by the scalar equation parser. The `dim_filtering` case passes a non-empty dimensions vector.
- **Implementation phase:** Phase 4, Task 1.

### AC1.5: Old 5 functions removed or converted to thin wrappers

- **Test type:** Human verification
- **Verification approach:** Code review of the Phase 1, Task 2 commit. The reviewer checks that:
  1. `IdentifierSetVisitor` struct and its `impl` block are deleted from `variable.rs`
  2. `identifier_set()` body is a single expression: `classify_dependencies(ast, dimensions, module_inputs).all`
  3. `init_referenced_idents()` body is: `classify_dependencies(ast, &[], None).init_referenced`
  4. `previous_referenced_idents()` body is: `classify_dependencies(ast, &[], None).previous_referenced`
  5. `lagged_only_previous_idents_with_module_inputs()` body is: `classify_dependencies(ast, &[], module_inputs).previous_only`
  6. `init_only_referenced_idents_with_module_inputs()` body is: `classify_dependencies(ast, &[], module_inputs).init_only`
  7. Function signatures and doc comments are preserved
- **Justification for human verification:** This criterion is about code structure (thin wrappers vs full implementations), not behavioral correctness. A test can verify that wrappers produce the same output as the old functions (which the existing tests already do via AC0.2), but cannot verify that the function body IS a thin wrapper rather than an independent reimplementation. Static analysis could theoretically check this, but a code review is the standard and practical approach.
- **Supplementary automated check:** All existing tests that call these wrapper functions (test_identifier_sets, test_init_only_referenced_idents, test_range_end_expressions_are_walked_in_init_previous_helpers) pass, confirming behavioral equivalence. After Phase 4, these tests are superseded by the matrix test.
- **Implementation phase:** Phase 1, Task 2.

### AC1.6: Nested PREVIOUS(PREVIOUS(x)) is handled correctly

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs`
- **Test name:** `test_classify_dependencies_matrix`
- **Specific case:** `nested_previous`: equation `PREVIOUS(PREVIOUS(x))`, expected `all={x}`, `previous_referenced={x}`, `previous_only={x}`, `init_referenced={}`, `init_only={}`
- **Rationale:** The `ClassifyVisitor` saves and restores `in_previous` around each PREVIOUS call. For nested PREVIOUS, the inner `x` is seen with `in_previous=true` (set by both outer and inner PREVIOUS). The inner PREVIOUS's arg (`x`) is recorded in `previous_referenced`. Since `x` never appears outside a PREVIOUS context, `non_previous` does not contain `x`, so `previous_only = previous_referenced - non_previous = {x}`.
- **Implementation phase:** Phase 4, Task 1 (test). Phase 1, Task 1 (implementation of save/restore flag logic).

---

## AC2: Simplified db.rs consumption

### AC2.1: variable_direct_dependencies_impl calls classify_dependencies exactly twice

- **Test type:** Human verification
- **Verification approach:** Code review of the Phase 2, Task 2 commit. The reviewer checks that:
  1. The non-Module arm of `variable_direct_dependencies_impl` contains exactly two `classify_dependencies` calls: one for `lowered.ast()` (dt AST) and one for `lowered.init_ast()` (init AST)
  2. No calls to the old wrapper functions (`identifier_set`, `init_referenced_idents`, etc.) remain in this function
  3. The `VariableDeps` struct is populated by mapping `DepClassification` fields directly (dt_classification.all -> dt_deps, init_classification.all -> initial_deps, etc.)
  4. The `extract_implicit_var_deps` call remains (it is a separate function, not replaced)
- **Justification for human verification:** The "exactly twice" constraint is a structural property of the code, not a behavioral property. Tests verify that the output is correct (AC2.3 via integration tests), but cannot verify the internal call count without instrumenting the function. Code review is the appropriate verification method for internal structure constraints.
- **Supplementary automated check:** AC2.3 (integration tests) confirms that the behavioral output of the function is identical. If the function produced incorrect `VariableDeps` due to wrong call structure, the integration tests would fail.
- **Implementation phase:** Phase 2, Task 2.

### AC2.2: extract_implicit_var_deps calls classify_dependencies exactly twice

- **Test type:** Human verification
- **Verification approach:** Code review of the Phase 2, Task 3 commit. The reviewer checks that:
  1. The `.map(|implicit_var| { ... })` closure contains exactly two `classify_dependencies` calls: one for `lowered.ast()` and one for `lowered.init_ast()`
  2. No calls to the old wrapper functions remain in this function
  3. `ImplicitVarDeps` fields map from `DepClassification` fields (dt_classification.all -> dt_deps, init_classification.previous_only -> dt_previous_referenced_vars, etc.)
  4. The Module early-return path is unchanged
- **Justification for human verification:** Same rationale as AC2.1 -- structural property, not behavioral.
- **Supplementary automated check:** Integration tests with SMOOTH/DELAY models exercise `extract_implicit_var_deps` through the full compilation pipeline. Incorrect implicit var deps would cause simulation output mismatches.
- **Implementation phase:** Phase 2, Task 3.

### AC2.3: Pruning logic produces identical dependency graphs

- **Test type:** Integration
- **Test file:** `src/simlin-engine/tests/simulate.rs` (primary), plus all other integration tests under `src/simlin-engine/tests/`
- **Verification command:** `cargo test -p simlin-engine --features file_io`
- **Rationale:** The pruning logic in `model_dependency_graph_impl` consumes `VariableDeps` fields for ordering decisions. If the refactored population of `VariableDeps` changes any field values, the dependency graph changes, which changes simulation execution order, which changes simulation outputs. The simulate.rs suite compares outputs against known-good values to 6+ decimal places, so even subtle ordering changes that affect numerics are caught.
- **Why not a dedicated graph-comparison test:** The dependency graph is an intermediate representation. Comparing it directly would require serializing and diffing the graph structure, which is fragile and adds maintenance burden. The simulation output comparison is a more robust end-to-end check that catches both graph structure changes and any downstream effects.
- **Implementation phase:** Phase 2, Tasks 2-3. Verified by running the pre-existing integration tests.

---

## AC3: Authoritative module-backed classifier

### AC3.1: collect_module_idents() and builtins_visitor use same predicate

- **Test type:** Human verification
- **Verification approach:** Code review of the Phase 3, Task 1 commit. The reviewer checks that:
  1. A `pub(crate) fn is_stdlib_module_function(func_name: &str) -> bool` exists in `src/simlin-engine/src/builtins.rs`
  2. `equation_is_stdlib_call()` in `src/simlin-engine/src/model.rs` calls `crate::builtins::is_stdlib_module_function(...)` instead of inlining `crate::stdlib::MODEL_NAMES.contains(...)` and `matches!(... "delay" | "delayn" | "smthn")`
  3. `contains_stdlib_call()` in `src/simlin-engine/src/builtins_visitor.rs` calls `crate::builtins::is_stdlib_module_function(...)` instead of inlining the same checks
  4. Each caller adds only its own structural logic on top (PREVIOUS arg-count check in model.rs; INIT inclusion and recursion in builtins_visitor.rs)
- **Justification for human verification:** This criterion requires verifying that two call sites reference the same function, which is a structural/code-organization property. Tests can verify that the behavior is correct (AC0 regression tests), but cannot verify that the correct behavior comes from a shared predicate rather than two independently correct implementations.
- **Supplementary automated check:** All integration tests pass (AC0.3), confirming that `collect_module_idents` and `builtins_visitor` produce correct results. If the shared predicate were wrong, module expansion would fail for affected models.
- **Implementation phase:** Phase 3, Task 1.

### AC3.2: No duplicated logic for module detection

- **Test type:** Human verification
- **Verification approach:** Code review of the Phase 3, Task 1 commit. The reviewer confirms:
  1. The string set `{"delay", "delayn", "smthn"} + MODEL_NAMES` appears in exactly one place: `is_stdlib_module_function` in `builtins.rs`
  2. No other location in the codebase contains `matches!(... "delay" | "delayn" | "smthn")` or equivalent checks against stdlib module names
  3. The `self.vars` runtime extension in `builtins_visitor.rs` is documented as using the same classification rule (incremental additions to the base set)
- **Verification aid:** A grep for `"delayn"` and `"smthn"` across the engine crate should return exactly one match (in `is_stdlib_module_function`), plus test code. Any additional matches indicate duplicated logic.
- **Justification for human verification:** Absence of duplication is a codebase-wide structural property. While the grep check is automatable, interpreting whether a match constitutes "duplicated logic" requires human judgment (test code, comments, and documentation may legitimately reference these names).
- **Implementation phase:** Phase 3, Task 1.

---

## AC4: Table-driven invariant tests

### AC4.1: Matrix covers all combinations (reference form x context)

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs`
- **Test name:** `test_classify_dependencies_matrix`
- **Matrix dimensions:**
  - Reference form (5): direct, PREVIOUS, INIT, mixed (current + lagged), both-lagged (PREVIOUS + INIT)
  - Context (4): scalar, IsModuleInput, ApplyToAll, subscript range
- **Expected coverage:** 5 x 4 = 20 core combinations, plus additional edge cases. The Phase 4 implementation plan enumerates 29 total cases including edge cases and variants.
- **Verification approach:** Count distinct (reference form, context) pairs in the test case array. Each of the 20 core cells must have at least one case.
- **Supplementary check:** The test case labels follow a naming convention that encodes both dimensions (e.g., `previous_a2a`, `init_ismoduleinput`, `both_lagged_scalar`), making the coverage matrix human-auditable.
- **Note on "phase" dimension:** The design specifies "phase (dt/initial)" as a matrix dimension. The implementation plan correctly observes that `classify_dependencies` is phase-agnostic -- it classifies a single AST regardless of whether the caller designates it dt or init. The "split by phase" behavior is in how `db.rs` assigns results from two separate calls. This is tested by the `split_phase_dt`/`split_phase_init` case pair (same AST, same results, demonstrating phase-agnosticism) and by Phase 5's differential checks (which verify the two calls produce consistent phase membership).
- **Implementation phase:** Phase 4, Task 1.

### AC4.2: Each cell asserts all 5 fields of DepClassification

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs`
- **Test name:** `test_classify_dependencies_matrix`
- **Verification approach:** The `DepTestCase` struct has 5 expected-value fields (`expected_all`, `expected_init_referenced`, `expected_previous_referenced`, `expected_previous_only`, `expected_init_only`), and the test runner asserts each one against the corresponding `DepClassification` field. All 5 assertions run for every case.
- **Structural invariant assertion:** The test runner also asserts `all (as strings) >= init_referenced UNION previous_referenced` for every case, which is the "fragment context" invariant from edge case 5.
- **Implementation phase:** Phase 4, Task 1.

### AC4.3: All 7 prior bug-fix edge cases have entries

- **Test type:** Unit
- **Test file:** `src/simlin-engine/src/variable.rs`
- **Test name:** `test_classify_dependencies_matrix`
- **Edge case mapping:**

| # | Edge Case | Test Case Label | Key Assertion |
|---|-----------|----------------|---------------|
| 1 | PREVIOUS feedback (`7a9db2a5`) | `previous_scalar` | `previous_only={b}`, `b` NOT in `non_previous` |
| 2 | Mixed current+lagged (`ae9f4ed9`) | `mixed_prev_current` | `previous_referenced={b}`, `previous_only={}` (b also outside PREVIOUS) |
| 3 | Split by phase (`09ae1b33`) | `split_phase_dt` / `split_phase_init` | Same AST produces same classification; phase split is in db.rs |
| 4 | INIT-only (`b0580011`) | `init_scalar` | `init_only={b}` |
| 5 | Fragment context (`55ebef55`) | `init_scalar` + structural invariant | `all={b}` includes INIT arg; invariant asserts `all >= init_referenced` |
| 6 | PREVIOUS+INIT combined (`c537bb2d`) | `both_lagged_scalar` | `init_only={b}`, `previous_only={b}` |
| 7 | Nested PREVIOUS (`0aecdfbb`) | `nested_previous` | `previous_only={x}` at both nesting levels |

- **Implementation phase:** Phase 4, Task 1.

---

## AC5: Differential checks

### AC5.1: Fragment/graph phase agreement for all integration test models

- **Test type:** Integration
- **Test file:** `src/simlin-engine/src/db_differential_tests.rs` (included as a `#[cfg(test)]` module from `db.rs`)
- **Test name:** `test_fragment_phase_agreement_integration_models`
- **Test requires:** `file_io` feature flag (for loading model files from disk)
- **Verification command:** `cargo test -p simlin-engine --features file_io test_fragment_phase_agreement_integration`
- **Approach:** For each variable in each model:
  1. Compile the variable via `compile_var_fragment` (fragment path)
  2. Query the variable's membership in `model_dependency_graph` runlists (full compile path)
  3. Assert the implication: if the fragment produced bytecodes for a phase, the variable is in that phase's runlist
- **Model selection:** 15-20 representative XMILE/STMX models from `tests/simulate.rs:TEST_MODELS`, covering: simple stocks/flows/auxes, SMOOTH/DELAY/TREND stdlib modules, 1D/2D/3D arrays, module-backed models, and eval-order edge cases.
- **Implication direction:** The check asserts `fragment has bytecodes => in runlist`. The reverse (`in runlist => fragment has bytecodes`) is not asserted because variables with no equation or empty compilation output may be in runlists without producing bytecodes.
- **Implementation phase:** Phase 5, Tasks 1-2.

### AC5.2: Synthetic models pass differential check

- **Test type:** Unit (no file I/O required)
- **Test file:** `src/simlin-engine/src/db_differential_tests.rs`
- **Test name:** `test_fragment_phase_agreement_synthetic_*` (one test per synthetic model, or a parameterized test)
- **Verification command:** `cargo test -p simlin-engine test_fragment_phase_agreement_synthetic`
- **Synthetic models:**

| Model | Variables | Tests |
|-------|-----------|-------|
| PREVIOUS feedback | `x = TIME`, `y = PREVIOUS(x) + 1` | PREVIOUS-only dep does not create same-step ordering |
| INIT-only deps | `x = TIME`, `y = INIT(x) + 1` | INIT-only dep places x in initials but not dt ordering |
| Nested builtins | `x = TIME`, `z = PREVIOUS(PREVIOUS(x))` | Implicit helper vars have consistent phases |
| Module-backed (SMOOTH) | `x = TIME`, `y = SMTH1(x, 1, x)` | Stdlib module expansion creates consistent phases |
| Mixed all three | `x = TIME`, `y = PREVIOUS(x) + INIT(x) + x` | Bare `x` reference creates same-step dep despite PREVIOUS/INIT |

- **Construction method:** Each model is built as a `datamodel::Project` with `datamodel::Model` and `datamodel::Variable` structs, compiled via `sync_from_datamodel`, then checked with `assert_fragment_phase_agreement`.
- **Implementation phase:** Phase 5, Task 3.

### AC5.3: Future disagreements are caught

- **Test type:** Structural property of AC5.1 and AC5.2 tests
- **Test file:** `src/simlin-engine/src/db_differential_tests.rs`
- **Verification approach:** This criterion is a liveness property ("if a bug is introduced, the test catches it"), not a specific test case. It is satisfied by the structural design of `assert_fragment_phase_agreement`:
  1. The function iterates ALL variables in a model, not a hardcoded list. New variables added to any integration test model are automatically checked.
  2. The assertion checks a general invariant (fragment bytecodes imply runlist membership), not model-specific expected values. No test case needs updating when models change.
  3. The synthetic models (AC5.2) exercise the specific combinations most likely to expose disagreements.
- **Justification:** This is not a separately testable criterion but a property of the test design. It is verified by inspection: the test iterates model variables dynamically and asserts a general invariant. Any new variable that violates the invariant will be caught without test modifications.
- **Regression scenario:** If a future change to `classify_dependencies` causes it to misclassify a variable's phase membership, `compile_var_fragment` (which gates on runlist membership) and `model_dependency_graph` (which builds runlists from dependency classifications) will disagree. The differential check detects this as a `fragment has bytecodes but variable NOT in runlist` assertion failure.
- **Implementation phase:** Phase 5, Tasks 1-3 (the test infrastructure that provides this property).

---

## Cross-Cutting Verification

### Pre-commit hook as regression gate

The pre-commit hook (`scripts/pre-commit`) runs the full engine test suite (Rust formatting, clippy, unit tests, integration tests, WASM build, TypeScript tests, Python tests) on every commit. This means:

- AC0.1, AC0.2, AC0.3 are verified on every commit, not just phase boundaries
- Any test added in Phases 4-5 automatically becomes part of the pre-commit gate
- The implementation plan's instruction to never use `--no-verify` ensures this gate is always active

### Test file path summary

| Test Category | File Path | Feature Flag |
|---------------|-----------|-------------|
| Existing simulation tests (AC0.1) | `src/simlin-engine/tests/simulate.rs` | `file_io` |
| Existing engine unit tests (AC0.2) | `src/simlin-engine/src/**/*.rs` (inline tests) | none |
| Full integration suite (AC0.3) | `src/simlin-engine/tests/*.rs` | `file_io` |
| Table-driven matrix test (AC1, AC4) | `src/simlin-engine/src/variable.rs` | none |
| Differential check -- integration (AC5.1) | `src/simlin-engine/src/db_differential_tests.rs` | `file_io` |
| Differential check -- synthetic (AC5.2, AC5.3) | `src/simlin-engine/src/db_differential_tests.rs` | none |

### Human verification checklist

The following criteria require code review and cannot be fully automated. Each should be checked during PR review of the corresponding phase:

- [ ] **AC1.5** (Phase 1, Task 2): Old functions are thin wrappers, IdentifierSetVisitor deleted
- [ ] **AC2.1** (Phase 2, Task 2): variable_direct_dependencies_impl has exactly 2 classify_dependencies calls
- [ ] **AC2.2** (Phase 2, Task 3): extract_implicit_var_deps has exactly 2 classify_dependencies calls
- [ ] **AC3.1** (Phase 3, Task 1): Both callers use is_stdlib_module_function
- [ ] **AC3.2** (Phase 3, Task 1): No duplicated name-set logic (grep for "delayn"/"smthn" shows single source)
