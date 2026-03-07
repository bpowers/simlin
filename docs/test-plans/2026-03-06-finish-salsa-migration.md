# Human Test Plan: Finish Salsa Migration

**Implementation Plan:** `docs/implementation-plans/2026-03-06-finish-salsa-migration/`
**Date:** 2026-03-07
**Automated Coverage:** PASS (all 26 acceptance sub-criteria covered)

## Prerequisites

- Branch `finish-salsa-migration` checked out and built
- `cargo test` passes (2853 tests)
- Access to test model files in `test/` directory

## Manual Verification Checklist

### 1. Monolithic Code Removal (AC4)

- [ ] Verify `Project::from` is only available under `#[cfg(any(test, feature = "testing"))]`:
  ```bash
  grep -n "impl From<datamodel::Project> for Project" src/simlin-engine/src/project.rs
  ```
  Confirm the impl block is inside a `#[cfg(any(test, feature = "testing"))]` gate.

- [ ] Verify `Simulation::compile()` is deleted:
  ```bash
  grep -n "fn compile" src/simlin-engine/src/interpreter.rs
  ```
  Should not find a `compile` method on `Simulation`.

- [ ] Verify `compile_project` free function is deleted:
  ```bash
  grep -n "pub fn compile_project\b" src/simlin-engine/src/interpreter.rs
  ```
  Should return no results.

- [ ] Verify no production code (non-test, non-`testing` feature) calls `Project::from`:
  ```bash
  cargo check -p simlin-engine 2>&1 | head -20
  ```
  Should compile cleanly without the `testing` feature.

### 2. CLI End-to-End (AC5, AC7)

- [ ] Run a basic simulation:
  ```bash
  cargo run -p simlin-cli -- simulate test/predator_prey/predator_prey.xmile
  ```
  Should produce simulation output without errors.

- [ ] Run error reporting:
  ```bash
  cargo run -p simlin-cli -- simulate test/missing_var/missing_var.xmile 2>&1 || true
  ```
  Should report errors cleanly, not panic.

- [ ] Test equations mode:
  ```bash
  cargo run -p simlin-cli -- equations test/predator_prey/predator_prey.xmile
  ```
  Should output LaTeX equations for the model.

### 3. Layout Generation (AC7)

- [ ] Verify layout tests pass:
  ```bash
  cargo test -p simlin-engine layout
  ```
  All layout-related tests should pass.

### 4. LTM Loop Detection (AC7)

- [ ] Verify LTM simulation tests pass:
  ```bash
  cargo test -p simlin-engine --features testing --test simulate_ltm
  ```
  All LTM tests should pass.

### 5. Dimension Invalidation (AC8)

- [ ] Verify dimension invalidation tests pass:
  ```bash
  cargo test -p simlin-engine dimension_invalidation
  ```
  All 4 dimension invalidation tests should pass:
  - `scalar_immune_to_dimension_changes`
  - `different_dimension_variable_immune`
  - `same_dimension_variable_reparsed`
  - `maps_to_chain_triggers_reparse`

### 6. Cross-Package Integration (AC7)

- [ ] Run libsimlin tests:
  ```bash
  cargo test -p libsimlin
  ```
  All tests pass.

- [ ] Run simlin-cli tests:
  ```bash
  cargo test -p simlin-cli
  ```
  All tests pass.

- [ ] Run full engine test suite:
  ```bash
  cargo test -p simlin-engine --features testing
  ```
  All tests pass.

### 7. Incremental Compilation Correctness (AC1, AC2)

- [ ] Verify the incremental compilation integration test covers all test models:
  ```bash
  cargo test -p simlin-engine --features testing,file_io --test simulate -- incremental_compilation_covers_all_models
  ```
  Should pass, confirming every test model compiles identically through the incremental path.

### 8. No Catch-Unwind in Incremental Paths (AC3)

- [ ] Verify no `catch_unwind` remains in engine source (excluding test code):
  ```bash
  grep -rn "catch_unwind" src/simlin-engine/src/ --include="*.rs" | grep -v test | grep -v "_tests.rs"
  ```
  Should return no results (all `catch_unwind` sites were removed).
