# Finish Salsa Migration Design

## Summary

Simlin's simulation engine has two parallel compilation pipelines: a monolithic
path (`Project::from` / `Simulation::compile`) that rebuilds everything from
scratch, and an incremental path (`compile_project_incremental`) built on salsa
tracked functions that recompiles only what changed. The incremental path is
already the production default for patch validation and interactive editing, but
the monolithic path persists as a fallback in the layout subsystem, causal-graph
analysis, the CLI, and most test code. This document plans the work to make the
incremental path the sole compilation pipeline, then delete the monolithic code
entirely.

The migration proceeds in seven phases. First, already-fixed issues around
module compilation gaps and error handling are verified and closed. Next,
defensive `catch_unwind` wrappers left over from when the incremental path could
panic are removed. The parse-context threading is unified so every variable is
parsed with module-identity awareness (preventing incorrect opcode selection for
module-backed functions like SMOOTH). Remaining production callers -- layout LTM
detection, model analysis, and the CLI -- are ported to accept a salsa database
instead of constructing a monolithic `Project`. Tests are migrated next, and
then the monolithic infrastructure (legacy error fields, manual dependency
caching, the `compile_project` free function, and `Simulation::compile`) is
deleted. An optional final phase narrows salsa invalidation so dimension changes
only re-parse variables that actually reference the affected dimensions.

## Definition of Done

1. **Incremental path handles all model types**: The 4 module/builtin gaps in the incremental compilation path (#295) are fixed -- module variables receive stock-phase bytecodes, implicit variables from builtin expansion get layout slots, module_models is populated in compile_var_fragment, and module input sets are differentiated per instance. The deprecated monolithic compiler (`Simulation::compile`, `compile_project`) is no longer needed as a fallback.

2. **Incremental path never panics on malformed models**: The incremental compiler returns clean `Result::Err` for models it cannot compile (e.g., C-LEARN with unsupported macros) instead of panicking (#363). `catch_unwind` wrappers are removed where the underlying panic is fixed.

3. **Module-aware parse context is cleanly decoupled**: `parse_source_variable` threading of `module_idents` is architecturally clean (#372, TD20) -- correctness-sensitive module context is explicit and local, not an implicit global dependency that destabilizes unrelated variable caches.

4. **Monolithic compilation path removed**: `compile_project`, `Simulation::compile()`, `Module::compile()`, `build_metadata`, and `compile_simulation` are deleted. All bytecode compilation goes through `compile_project_incremental` / `assemble_simulation`. Legacy error fields on Variable/ModelStage types are removed (TD17).

5. **Dependency analysis fully routed through salsa**: `set_dependencies_cached` and `all_deps` in model.rs are removed. All dependency analysis goes through `variable_direct_dependencies` and `model_dependency_graph` tracked functions (#294).

6. **Single sync path**: `sync_from_datamodel` (fresh inputs) is removed or made internal to the incremental path. All production callers use `sync_from_datamodel_incremental` (#290, #292).

7. **All existing tests pass**: Simulation tests, LTM tests, and integration tests produce identical numerical results. No new issues opened.

8. **TD18 (dimension-granularity invalidation)**: Included if the fix is a straightforward tracked function addition; deferred otherwise.

## Acceptance Criteria

### finish-salsa-migration.AC1: Incremental path handles all model types
- **finish-salsa-migration.AC1.1 Success:** Models with module variables compile through `compile_project_incremental` and produce identical simulation results to the monolithic path.
- **finish-salsa-migration.AC1.2 Success:** Models with SMOOTH/DELAY/TREND builtins compile through the incremental path with correct layout slots for implicit variables.
- **finish-salsa-migration.AC1.3 Success:** Multiple instances of the same sub-model with different input wirings produce distinct compiled module entries.
- **finish-salsa-migration.AC1.4 Success:** `test_incremental_compile_smooth_over_module_output` and `test_incremental_compile_distinguishes_module_input_sets` pass (existing coverage).

### finish-salsa-migration.AC2: Incremental path never panics on malformed models
- **finish-salsa-migration.AC2.1 Success:** Compiling a model with unknown builtins (e.g., Vensim macros) through `compile_project_incremental` returns `Err(NotSimulatable)`, not a panic.
- **finish-salsa-migration.AC2.2 Success:** Compiling a model with missing module references returns `Err`, not a panic.
- **finish-salsa-migration.AC2.3 Success:** `catch_unwind` wrappers removed from benchmarks (`benches/compiler.rs`), tests (`tests/simulate.rs`), and incremental layout paths (`layout/mod.rs`).
- **finish-salsa-migration.AC2.4 Success:** `compile_project_incremental` docstring accurately describes current behavior (no stale monolithic-fallback claim).

### finish-salsa-migration.AC3: Module-aware parse context unified
- **finish-salsa-migration.AC3.1 Success:** Only `parse_source_variable_with_module_context` exists; the plain `parse_source_variable` variant is deleted.
- **finish-salsa-migration.AC3.2 Success:** `PREVIOUS(x)` where `x = SMTH1(input, 1)` compiles to module expansion (not `LoadPrev`) through the salsa incremental path.
- **finish-salsa-migration.AC3.3 Success:** Editing an unrelated variable does not trigger re-parse of variables in the same model (salsa cache stability preserved).

### finish-salsa-migration.AC4: Monolithic compilation path removed
- **finish-salsa-migration.AC4.1 Success:** `compile_project` (free function in interpreter.rs) does not exist.
- **finish-salsa-migration.AC4.2 Success:** `Simulation::compile()` does not exist.
- **finish-salsa-migration.AC4.3 Success:** `set_dependencies_cached`, `set_dependencies`, `all_deps` do not exist in model.rs.
- **finish-salsa-migration.AC4.4 Success:** `Project::from` impl and `Project::base_from` do not exist.
- **finish-salsa-migration.AC4.5 Success:** Legacy `errors`/`unit_errors` fields removed from `Variable`, `ModelStage0`, `ModelStage1`.
- **finish-salsa-migration.AC4.6 Success:** `Simulation::new()` + `run_to_end()` (AST interpreter) still works for cross-validation.

### finish-salsa-migration.AC5: Dependency analysis routed through salsa
- **finish-salsa-migration.AC5.1 Success:** All dependency analysis in the compilation pipeline goes through `variable_direct_dependencies` and `model_dependency_graph` tracked functions.
- **finish-salsa-migration.AC5.2 Success:** No production or test code calls `all_deps` or `set_dependencies`.

### finish-salsa-migration.AC6: Single sync path
- **finish-salsa-migration.AC6.1 Success:** No production caller invokes `sync_from_datamodel` directly; all go through `sync_from_datamodel_incremental`.
- **finish-salsa-migration.AC6.2 Success:** `sync_from_datamodel` remains as an internal bootstrap function called by `sync_from_datamodel_incremental` when `prev_state` is `None`.

### finish-salsa-migration.AC7: All existing tests pass
- **finish-salsa-migration.AC7.1 Success:** All tests in `tests/simulate*.rs` pass with identical numerical results.
- **finish-salsa-migration.AC7.2 Success:** All LTM tests pass with identical results.
- **finish-salsa-migration.AC7.3 Success:** All libsimlin integration tests pass.
- **finish-salsa-migration.AC7.4 Success:** `cargo test -p simlin-engine` and `cargo test -p libsimlin` both pass cleanly.

### finish-salsa-migration.AC8: Dimension-granularity invalidation (TD18)
- **finish-salsa-migration.AC8.1 Success:** Changing dimension A does not trigger re-parse of a scalar variable.
- **finish-salsa-migration.AC8.2 Success:** Changing dimension A does not trigger re-parse of a variable that only references dimension B.
- **finish-salsa-migration.AC8.3 Success:** Changing dimension A does trigger re-parse of a variable that references dimension A (including via `maps_to` chains).

## Glossary

- **salsa**: An incremental computation framework for Rust. Functions are declared as "tracked," and salsa automatically memoizes their results and invalidates only what needs recomputation when inputs change. Foundation of the incremental compilation pipeline.
- **tracked function**: A salsa concept -- a pure function whose inputs and outputs are recorded by the framework. When an input changes, salsa re-executes only the tracked functions whose inputs were affected.
- **interned type**: A salsa concept where structurally identical values are deduplicated and assigned a stable integer ID. Used here for `ModuleIdentContext` so that an unchanged set of module identifiers produces the same ID across revisions.
- **monolithic compilation path**: The original pipeline (`Project::from`, `Simulation::compile`, `compile_project`) that rebuilds the entire project from scratch on every change. Being replaced by the incremental path.
- **incremental compilation path**: The salsa-based pipeline (`compile_project_incremental` / `assemble_simulation`) that recompiles only variables whose inputs have changed. Already the production default; this plan makes it the only path.
- **LTM (Loops That Matter)**: A system dynamics analysis technique that identifies which feedback loops in a model are driving behavior at each point in time. Requires compiling synthetic instrumentation variables into the model.
- **module variable**: A variable whose equation expands into an entire sub-model (e.g., SMOOTH, DELAY, TREND are implemented as stdlib module definitions). Module variables occupy multiple layout slots and require special handling during compilation.
- **implicit variable**: A variable automatically generated during builtin expansion -- for example, `SMOOTH(x, 5)` creates a hidden stock variable. These must receive layout slots in the VM memory map.
- **layout slot**: A position in the VM's flat memory array assigned to a variable. The VM addresses all model state by integer offset into this array.
- **`LoadPrev` / `LoadInitial`**: VM opcodes for the `PREVIOUS()` and `INIT()` builtins. `LoadPrev` reads a variable's value from the prior timestep; `LoadInitial` reads its value from the first timestep. Only valid for simple scalar variables -- module-backed arguments must fall through to module expansion.
- **`catch_unwind`**: A Rust standard library function that catches panics at a boundary, converting them to `Result::Err`. Used as a defensive wrapper when the incremental path could panic; being removed as the path now returns clean errors.
- **module_idents / ModuleIdentContext**: The set of variable names in a model that will expand into sub-model modules. Threaded through parsing so that `PREVIOUS(module_var)` correctly routes to module expansion rather than `LoadPrev`.
- **`sync_from_datamodel` / `sync_from_datamodel_incremental`**: Functions that synchronize a datamodel `Project` into salsa input types. The incremental variant uses salsa setters to update only changed fields. The non-incremental variant is the bootstrap path for first sync.
- **`SourceProject` / `SourceModel` / `SourceVariable`**: Salsa input types that mirror the datamodel structures. Setting fields on these inputs is how external changes enter the incremental pipeline.
- **`CompilationDiagnostic`**: A salsa accumulator that collects per-variable diagnostics during incremental compilation. Replaces the legacy pattern of storing errors directly on `Variable` and `ModelStage` structs.
- **`PersistentSyncState`**: A structure that bridges salsa database lifetimes across FFI revision boundaries by storing lifetime-erased `salsa::Id` values.
- **dimension / subscript**: System dynamics array notation. A dimension defines named index sets (e.g., `Region: East, West, North`); variables can be arrayed over one or more dimensions.
- **`maps_to` chain**: A dimension mapping where dimension A's elements correspond to elements in dimension B. Relevant here because changing a mapped dimension must invalidate variables referencing either end of the chain.
- **TD17, TD18, TD20**: Entries in the project's tech-debt tracking document (`docs/tech-debt.md`), referenced by number throughout this plan.

## Architecture

The incremental compilation pipeline (`compile_project_incremental` in
`src/simlin-engine/src/db.rs`) is already the production compilation path for
patch validation and simulation. Investigation reveals that all four module/
builtin gaps documented in #295 have been fixed, the incremental path handles
errors cleanly without panicking, and the module-aware parse context
architecture proposed in #372 is largely implemented.

The remaining work is consolidation: migrating the handful of production callers
that still use the monolithic `Project::from` path (layout LTM detection,
causal-graph analysis, CLI error reporting), then deleting the monolithic code
and its supporting infrastructure (`set_dependencies`, `all_deps`, legacy error
fields, `Simulation::compile`, `compile_project`).

### Parse Context Unification

`parse_source_variable` (no module context, `module_idents = None`) is replaced
everywhere by `parse_source_variable_with_module_context`. The plain variant is
deleted. Since `ModuleIdentContext` is a salsa interned type, content-based
comparison means the interned ID is stable when the set of module variables
doesn't change -- no spurious invalidation.

### Caller Migration

Production callers of the monolithic path migrate to accept `&dyn Db` +
`SourceProject`:

- **Layout** (`src/simlin-engine/src/layout/mod.rs`):
  `try_detect_ltm_loops_monolithic` is deleted;
  `try_detect_ltm_loops_incremental` (which already exists) becomes the sole
  path.
- **Analysis** (`src/simlin-engine/src/analysis.rs`): `analyze_model` gains a
  `db: &dyn Db` parameter and uses salsa causal graph functions instead of
  `Project::from`.
- **CLI** (`src/simlin-cli/src/main.rs`): Creates a `SimlinDb`, uses
  `sync_from_datamodel` + `compile_project_incremental` for all modes.
- **Project LTM methods** (`src/simlin-engine/src/project.rs`): `with_ltm()`
  and `with_ltm_all_links()` are deleted -- LTM is always-on in the
  incremental path.

### Test Migration

`TestProject` in `src/simlin-engine/src/test_common.rs` gains an incremental
compilation method. All ~50+ test callers of `Simulation::compile()` migrate
to the incremental path. `Project::from` is deleted entirely (not retained as
test-only).

### Dimension-Granularity Invalidation (TD18)

A `variable_relevant_dimensions` tracked function extracts dimension names
from `SourceEquation` variants (`ApplyToAll(dim_names, _)`,
`Arrayed(dim_names, _)`, `Scalar(_)`). `parse_source_variable_with_module_context`
filters `project.dimensions(db)` to only relevant dimensions before passing
them to `parse_var_with_module_context`. Scalar variables get an empty
dimension set, making them immune to dimension changes. Transitive `maps_to`
chains are included in the relevant set for array variables with stdlib calls.

## Existing Patterns

### Incremental Pipeline

The salsa-based incremental pipeline is fully operational for all model types.
Per-variable tracked functions (`parse_source_variable_with_module_context`,
`variable_direct_dependencies`, `compile_var_fragment`) with symbolic bytecode
and late assembly (`assemble_module`, `assemble_simulation`). This design
removes the parallel monolithic path rather than introducing new patterns.

### Error Handling

The incremental path returns `Result::Err` for all error conditions. The
`CompilationDiagnostic` salsa accumulator collects per-variable diagnostics.
`collect_all_diagnostics` is the sole error collection path in production
(`apply_project_patch_internal` in `src/libsimlin/src/patch.rs`).

### Module Context Threading

`ModuleIdentContext` (interned salsa type) and
`model_module_ident_context` (tracked function) provide stable, per-model
module identity sets. `compile_var_fragment` and
`variable_direct_dependencies_with_context` already use the module-context
variant of parsing.

### Persistent Sync State

`PersistentSyncState` in `src/simlin-engine/src/db.rs` bridges salsa
lifetimes across FFI revision boundaries by erasing `'db` lifetimes via
`salsa::Id`. `sync_from_datamodel_incremental` uses salsa setters to update
only changed fields, triggering minimal invalidation.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Verify and Close Already-Fixed Issues

**Goal:** Confirm that #290 and #295 are resolved in the current codebase and
close them.

**Components:**
- Issue #290: Verify `sync_from_datamodel_incremental` in
  `src/simlin-engine/src/db.rs` uses salsa setters for in-place updates and
  is used by `apply_project_patch_internal` in
  `src/libsimlin/src/patch.rs`
- Issue #295: Verify all 4 module gaps are fixed:
  - `compile_var_fragment` generates stock bytecodes for module-kind variables
    (the `is_stock || is_module` guard)
  - `compute_layout` allocates slots for implicit variables from
    SMOOTH/DELAY/TREND
  - `model_module_map` populates `module_models` in `compile_var_fragment`
  - `enumerate_module_instances` differentiates input sets per model instance
- Confirm existing tests cover these cases (`test_incremental_compile_smooth_over_module_output`,
  `test_incremental_compile_distinguishes_module_input_sets` in
  `src/simlin-engine/src/db_tests.rs`)

**Dependencies:** None (first phase)

**Done when:** Both issues confirmed resolved with evidence from existing
tests. Commit closes them with `Fix #290` and `Fix #295`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Remove Vestigial `catch_unwind` Wrappers

**Goal:** Remove `catch_unwind` from paths where the incremental compiler
returns clean errors, confirming #363 is resolved.

**Components:**
- `src/simlin-engine/benches/compiler.rs:75` -- remove `catch_unwind` around
  `sync_from_datamodel_incremental` + `compile_project_incremental`; let
  `Result` propagate
- `src/simlin-engine/tests/simulate.rs:1294` -- remove `catch_unwind` in
  `incremental_compilation_covers_all_models`; use `Result` matching
- `src/simlin-engine/src/layout/mod.rs:2041,2061` -- remove `catch_unwind`
  around salsa queries and incremental compile in
  `try_detect_ltm_loops_incremental`
- Update stale docstring on `compile_project_incremental` (db.rs:5798-5800)
  that incorrectly claims monolithic fallback

**Deferred to Phase 4:**
- `src/simlin-engine/src/layout/mod.rs:1874` (wraps monolithic `Project::from`)
- `src/simlin-engine/src/layout/mod.rs:2129` (wraps monolithic LTM pipeline)
- `src/simlin-engine/src/analysis.rs:124` (wraps monolithic LTM pipeline)

**Dependencies:** Phase 1

**Done when:** 4 `catch_unwind` sites removed. Benchmarks, tests, and
incremental layout paths handle errors via `Result`. Commit closes #363
with `Fix #363`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Unify Parse Context

**Goal:** Replace `parse_source_variable` (no module context) with
`parse_source_variable_with_module_context` everywhere, then delete the
plain variant. Close #372 and TD20.

**Components:**
- `src/simlin-engine/src/db.rs` -- delete `parse_source_variable` (the
  variant that passes `module_idents = None`); update all callers to use
  `parse_source_variable_with_module_context`
- Callers to audit and migrate: `model_implicit_var_info`,
  `collect_model_diagnostics`, any diagnostic paths that use the plain
  variant
- Add test model: `x = SMTH1(input, 1)` and `y = PREVIOUS(x)`, verify `y`
  compiles correctly via incremental path (not LoadPrev but module expansion)
- Document parse-context dependency model in code comments

**Dependencies:** Phase 1

**Done when:** Only `parse_source_variable_with_module_context` exists. TD20
test passes. Commit closes #372 with `Fix #372`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Migrate Remaining Monolithic Callers

**Goal:** All production callers use the incremental path. No production code
calls `Project::from`, `with_ltm()`, or `Simulation::compile()`.

**Components:**
- `src/simlin-engine/src/layout/mod.rs` -- delete
  `try_detect_ltm_loops_monolithic` and `try_compile_model` (the monolithic
  fallbacks); ensure `try_detect_ltm_loops_incremental` is the sole LTM path;
  remove remaining `catch_unwind` wrappers (deferred from Phase 2)
- `src/simlin-engine/src/analysis.rs` -- `analyze_model` gains `db: &dyn Db`
  + `SourceProject` parameters; uses salsa causal graph functions
  (`model_causal_edges`, `model_detected_loops`) instead of `Project::from`;
  remove `catch_unwind`
- `src/simlin-cli/src/main.rs` -- `run_datamodel_with_errors` uses
  `sync_from_datamodel` + `collect_all_diagnostics` for error reporting;
  `simulate()` with LTM uses incremental path; `--equations` mode walks
  `SourceModel` variables
- `src/simlin-engine/src/project.rs` -- delete `with_ltm()` and
  `with_ltm_all_links()` methods
- Thread `&dyn Db` through libsimlin callers of layout/analysis
  (`src/libsimlin/src/`)

**Dependencies:** Phases 2, 3

**Done when:** No production code path calls `Project::from`,
`Simulation::compile()`, `with_ltm()`, or `with_ltm_all_links()`. All
layout, analysis, and CLI functionality works via incremental path. Commit
closes #292 with `Fix #292`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Migrate Tests to Incremental Path

**Goal:** All test code uses the incremental compilation path. No test calls
`Simulation::compile()` or `Project::from` for compilation.

**Components:**
- `src/simlin-engine/src/test_common.rs` -- add `TestProject::compile_incremental()`
  method that creates a `SimlinDb`, syncs, and calls
  `compile_project_incremental`; add `TestProject::assert_compiles_incremental()`
  and `TestProject::run_vm_incremental()` helpers
- `src/simlin-engine/tests/simulate.rs` -- migrate `compile_vm_monolithic`,
  `simulate_path_with`, `simulate_mdl_path`, and related helpers to use
  incremental compilation
- `src/simlin-engine/tests/simulate_ltm.rs` -- migrate LTM tests from
  monolithic `Simulation::compile()` to incremental path
- `src/simlin-engine/tests/vm_alloc.rs` -- migrate VM allocation tests
- `src/simlin-engine/src/vm.rs` (`#[cfg(test)]` blocks) -- migrate ~17 test
  sites
- `src/simlin-engine/src/compiler/dimensions.rs`,
  `src/simlin-engine/src/compiler/symbolic.rs` -- migrate compiler tests
- `src/simlin-engine/src/ltm.rs`, `src/simlin-engine/src/ltm_augment.rs` --
  migrate LTM module tests
- `src/simlin-engine/src/project.rs` (`#[cfg(test)]`) -- migrate project tests
- `src/libsimlin/src/errors.rs` (`#[cfg(test)]`) -- migrate error formatting
  tests

**Dependencies:** Phase 4

**Done when:** Zero test callers of `Simulation::compile()` or
`compile_project` remain. All tests pass with identical numerical results.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Delete Monolithic Compilation Path

**Goal:** Remove all monolithic compilation code, dependency analysis, and
legacy error fields. Close #294 and TD17.

**Components:**
- `src/simlin-engine/src/interpreter.rs` -- delete `compile_project` free
  function, `Simulation::compile()`, `calc_flattened_offsets`; retain
  `Simulation::new()` + `run_to_end()` (AST interpreter) and `Module::new`
- `src/simlin-engine/src/model.rs` -- delete `set_dependencies_cached`,
  `set_dependencies`, `all_deps`, and associated helper functions
  (`build_runlist` if only used by these)
- `src/simlin-engine/src/project.rs` -- delete `Project::from` impl,
  `Project::base_from`, `ModelStage0::new_cached`,
  `Project::from_with_model_cb`; remove legacy `errors`/`unit_errors` fields
  from `Variable`, `ModelStage0`, `ModelStage1` (TD17)
- `src/simlin-engine/src/compiler/mod.rs` -- delete `build_metadata` if no
  longer needed by `Module::new` (verify first); remove any dead code
  exposed by deletion
- `src/libsimlin/src/errors.rs` -- delete `collect_project_errors` and
  `collect_formatted_issues` (struct-field error walking)
- `src/simlin-engine/src/test_common.rs` -- remove
  `TestProject::assert_compiles` and `TestProject::run_vm` (the monolithic
  variants)
- Remove deprecated markers and stale docstrings referencing the monolithic
  path

**Dependencies:** Phase 5

**Done when:** `compile_project`, `Simulation::compile()`,
`set_dependencies_cached`, `set_dependencies`, `all_deps` no longer exist.
Legacy error fields removed. `cargo build` succeeds with no dead code
warnings. All tests pass. Commit closes #294 with `Fix #294`.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Dimension-Granularity Invalidation (TD18)

**Goal:** Narrow salsa invalidation so dimension changes only re-parse
variables that reference the changed dimensions.

**Components:**
- `src/simlin-engine/src/db.rs` -- add `variable_relevant_dimensions`
  tracked function that extracts dimension names from `SourceEquation`
  variants; returns `BTreeSet<String>` (empty for `Scalar`)
- `src/simlin-engine/src/db.rs` -- modify
  `parse_source_variable_with_module_context` to filter
  `project.dimensions(db)` through `variable_relevant_dimensions` before
  passing to `parse_var_with_module_context`
- Handle transitive `maps_to` chains: if a variable references dimension A
  which maps to dimension B, include B in the relevant set
- `src/simlin-engine/src/variable.rs` -- `parse_var_with_module_context`
  receives filtered dimensions; `instantiate_implicit_modules` uses filtered
  `DimensionsContext`

**Dependencies:** Phase 3 (parse context unification)

**Done when:** Adding/modifying an unrelated dimension does not trigger
re-parse of scalar variables or variables referencing only other dimensions.
Verified by test: model with dimensions A and B, change dimension A, confirm
variable using only B is not re-parsed (salsa event logging or cache-hit
assertion).
<!-- END_PHASE_7 -->

## Additional Considerations

### Interpreter Preservation

The AST-walking interpreter (`Simulation::new()` + `run_to_end()`) is
retained as a reference implementation for cross-validation in tests.
`Module::new` (which builds `Vec<Expr>` runlists) is kept since the
interpreter depends on it. The interpreter does not use any of the deleted
monolithic compilation code.

### `sync_from_datamodel` Remains as Bootstrap

`sync_from_datamodel` is not deleted -- it is the correct bootstrap path
called by `sync_from_datamodel_incremental` when `prev_state` is `None`
(first sync). The DoD item about "single sync path" means no production
caller uses the fresh-input `sync_from_datamodel` directly; they all go
through `sync_from_datamodel_incremental`.

### Ignored Tests

Several `#[ignore]`d tests (`simulates_clearn`, `simulates_except*`,
`simulates_delayfixed*`, `simulates_get_direct_*`) use the monolithic path
but are ignored due to unrelated feature gaps (macro expansion, cross-dimension
mapping, ring-buffer semantics, external data loading). These tests are
migrated to the incremental path in Phase 5 but remain `#[ignore]`d for
their original reasons.

### PR Commit Convention

All commits that resolve issues use `Fix #N` in the commit message body to
trigger GitHub auto-close. Issues are closed in the phase where the fix is
verified, not deferred to the final phase.
