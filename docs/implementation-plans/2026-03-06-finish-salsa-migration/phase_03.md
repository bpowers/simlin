# Finish Salsa Migration Implementation Plan

**Goal:** Make the incremental salsa compilation pipeline the sole compilation path, then delete the monolithic code.

**Architecture:** The salsa-based incremental pipeline (`compile_project_incremental` / `assemble_simulation`) is already the production default. This plan migrates remaining callers, tests, and infrastructure, then deletes the monolithic path (`Project::from`, `Simulation::compile`, `compile_project`).

**Tech Stack:** Rust, salsa (incremental computation framework)

**Scope:** 7 phases from original design (phases 1-7)

**Codebase verified:** 2026-03-06

---

## Acceptance Criteria Coverage

This phase implements and tests:

### finish-salsa-migration.AC3: Module-aware parse context unified
- **finish-salsa-migration.AC3.1 Success:** Only `parse_source_variable_with_module_context` exists; the plain `parse_source_variable` variant is deleted.
- **finish-salsa-migration.AC3.2 Success:** `PREVIOUS(x)` where `x = SMTH1(input, 1)` compiles to module expansion (not `LoadPrev`) through the salsa incremental path.
- **finish-salsa-migration.AC3.3 Success:** Editing an unrelated variable does not trigger re-parse of variables in the same model (salsa cache stability preserved).

---

## Phase 3: Unify Parse Context

**Phase type:** Functionality -- migrating callers, deleting a function, adding a test.

**Note on line numbers:** Line numbers referenced below are approximate and may shift as earlier phases modify files. Use function name search rather than relying on exact line numbers.

**Background:** Two tracked parse functions exist in `src/simlin-engine/src/db.rs`:
- `parse_source_variable` (line 742) -- passes `module_idents = None`, meaning module-backed variables (SMOOTH, DELAY, TREND) are not recognized as modules during parsing. This can cause `PREVIOUS(smooth_var)` to incorrectly compile to `LoadPrev` instead of module expansion.
- `parse_source_variable_with_module_context` (line 750) -- receives a `ModuleIdentContext` (salsa interned type) so module-backed variables are correctly identified.

Both delegate to `parse_source_variable_impl` (line 717). The goal is to migrate all callers to the `_with_module_context` variant, then delete the plain variant.

**Callers of plain `parse_source_variable` (from codebase investigation):**

Production callers:
- `db.rs:854` -- `variable_direct_dependencies_impl` fallback (when no module context provided)
- `db.rs:1549` -- `build_model_s0` (monolithic path, will be deleted in Phase 6)
- `db.rs:2894` -- `variable_dimensions`
- `db_analysis.rs:180` -- `model_causal_edges` (implicit variable traversal)
- `db_analysis.rs:375, 424, 434` -- causal analysis variable reconstruction helpers
- `db_ltm.rs:561` -- LTM implicit module input extraction

Test callers in `db_tests.rs`: ~16 sites (lines 485, 562, 568, 586, 662, 663, 675, 676, 2189, 2273, 2518, 2519, 2546, 2555, 2706, 2731)

**Note:** `model_implicit_var_info` (db.rs:998) and `collect_model_diagnostics` (db.rs:2182) already use the module-context variant or do not call parse functions directly -- no migration needed for those.

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Add test for PREVIOUS(SMTH1_var) simulation correctness

**Verifies:** finish-salsa-migration.AC3.2

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs`

**Implementation:**

Add a test `test_previous_of_module_backed_variable_compiles_correctly` that:
1. Creates a `TestProject` with variables:
   - `input` = some constant (e.g., `10`)
   - `x` = `SMTH1(input, 1)` (this is module-backed)
   - `y` = `PREVIOUS(x)` (should use module expansion, NOT `LoadPrev`)
2. Syncs to a `SimlinDb` via `sync_from_datamodel_incremental`
3. Compiles via `compile_project_incremental`
4. Runs the VM to completion
5. Asserts that `y` has the expected values (shifted-by-one-timestep smooth of input)

The key assertion: `y` should NOT equal `x` shifted by one timestep as a raw scalar (which `LoadPrev` would produce). Instead, it should equal the PREVIOUS of the smoothed value, which means both `x` and `y` involve module expansion.

Follow the pattern used in `test_incremental_compile_smooth_over_module_output` (db_tests.rs:4004) for how to set up the `SimlinDb`, sync, compile, and run.

**Testing:**
- finish-salsa-migration.AC3.2: Assert that PREVIOUS(x) where x=SMTH1(input,1) produces correct simulation results through the incremental path (not LoadPrev).

**Verification:**
```bash
cargo test -p simlin-engine test_previous_of_module_backed_variable_compiles_correctly -- --nocapture
```
Expected: Test passes with correct simulation values.

**Commit:** `engine: add test for PREVIOUS of module-backed variable`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Migrate production callers to parse_source_variable_with_module_context

**Verifies:** finish-salsa-migration.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (lines ~854, ~1549, ~2894)
- Modify: `src/simlin-engine/src/db_analysis.rs` (lines ~180, ~375, ~424, ~434)
- Modify: `src/simlin-engine/src/db_ltm.rs` (line ~561)

**Implementation:**

For each call site, obtain a `ModuleIdentContext` for the relevant model and pass it to `parse_source_variable_with_module_context` instead of `parse_source_variable`.

The standard pattern to obtain the context is:
```rust
let module_ident_context = module_ident_context_for_model(db, model, &[]);
```
or if extra module idents are needed:
```rust
let module_ident_context = model_module_ident_context(db, model, extra_idents);
```

**Migration details per call site:**

1. **db.rs:854** (`variable_direct_dependencies_impl` fallback): The function already has a `module_ident_context: Option<ModuleIdentContext>` parameter. The `None` case calls `parse_source_variable`. After this migration, make `module_ident_context` non-optional -- always require it. Update both callers of `variable_direct_dependencies_impl` to always pass a context. If a model reference is available, use `module_ident_context_for_model(db, model, &[])`.

2. **db.rs:1549** (`build_model_s0`): This is the monolithic path that will be deleted in Phase 6. For now, obtain the context from the model being built and pass it through. This lets us delete `parse_source_variable` without waiting for Phase 6.

3. **db.rs:2894** (`variable_dimensions`): This function just needs the parsed variable to extract dimension info. Thread a `ModuleIdentContext` parameter through and obtain it from the caller chain.

4. **db_analysis.rs:180, 375, 424, 434**: These causal analysis functions walk model variables. For each, obtain the model's module ident context once at the top of the function and pass it to all parse calls within.

5. **db_ltm.rs:561**: LTM implicit module input extraction. Obtain the model's context and use the `_with_module_context` variant.

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles and all tests pass.

**Commit:** `engine: migrate all callers to parse_source_variable_with_module_context`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Migrate test callers and delete parse_source_variable

**Verifies:** finish-salsa-migration.AC3.1

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs` (~16 call sites)
- Modify: `src/simlin-engine/src/db.rs` (delete `parse_source_variable` at lines 742-748)

**Implementation:**

1. In `db_tests.rs`, update each call to `parse_source_variable(db, var, project)` to use `parse_source_variable_with_module_context(db, var, project, module_ident_context)`. For test code, obtain the context via `module_ident_context_for_model(db, model, &[])` or `model_module_ident_context(db, model, vec![])`.

2. Delete the `parse_source_variable` function at db.rs lines 742-748.

3. Delete the `#[salsa::tracked]` attribute and function body. The shared `parse_source_variable_impl` at line 717 is still needed (called by `parse_source_variable_with_module_context`).

4. Consider renaming `parse_source_variable_with_module_context` to just `parse_source_variable` since it's now the only variant. This is optional -- if it makes the code cleaner, do it; if it causes churn in too many call sites, leave the longer name.

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles with no dead-code warnings for `parse_source_variable`. All tests pass.

**Commit:** `engine: delete parse_source_variable, unifying on module-context variant`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_TASK_4 -->
### Task 4: Verify salsa cache stability (AC3.3)

**Verifies:** finish-salsa-migration.AC3.3

**Files:**
- Read: `src/simlin-engine/src/db.rs` (ModuleIdentContext definition at line ~139)

**Implementation:**

This is a verification task, not an implementation task. The salsa cache stability property holds because:
- `ModuleIdentContext` is a `#[salsa::interned]` type (db.rs:139-143)
- Interned types use content-based comparison -- if the set of module variables in a model doesn't change, the interned ID is the same across revisions
- Therefore, editing an unrelated variable (which doesn't change the set of module variables) does not change the `ModuleIdentContext`, and `parse_source_variable_with_module_context` will return a cached result

Verify this reasoning by reading the `ModuleIdentContext` definition and the `model_module_ident_context` function. If the interning and tracked-function properties are as described, AC3.3 is satisfied structurally by salsa's design.

No code changes needed. If desired, add a test that:
1. Syncs a model, parses variable A
2. Edits an unrelated variable B's equation
3. Re-syncs incrementally
4. Asserts variable A's parse result is unchanged (via salsa event logging or by checking that `parse_source_variable_with_module_context` returns a reference-equal result)

**Verification:**
```bash
cargo test -p simlin-engine
```
Expected: All tests pass.

**Commit:** `engine: unify parse context on module-aware variant

Without module context, PREVIOUS(x) where x is backed by a stdlib module
(SMTH1, DELAY, TREND) incorrectly compiles to LoadPrev instead of
expanding the module. Unifying on the module-context variant ensures all
parse paths have correct module identity awareness.

Fix #372`
<!-- END_TASK_4 -->
