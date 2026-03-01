# Activate PREVIOUS/INIT Single-Opcode Compilation

## Summary

Simlin's simulation engine already contains scaffolding for `LoadPrev` and `LoadInitial` VM opcodes -- bytecode instructions, snapshot buffers, codegen paths, and execution handlers -- but a gate in `builtins_visitor.rs` forces all `PREVIOUS(x)` and `INIT(x)` calls through stdlib module expansion instead. Module expansion synthesizes an entire stock-and-flow submodel for each call, which is expensive and adds significant complexity to LTM (Loops That Matter) compilation.

This design activates the existing opcode infrastructure by replacing the blanket gate with an arity-based check: 1-argument `PREVIOUS(x)` and `INIT(x)` compile directly to their respective opcodes, while 2-argument `PREVIOUS(x, init_val)` continues using module expansion. The work is broken into five phases: activating the gate and fixing the `prev_values` snapshot buffer (Phase 1), extending the Initials runlist so `INIT`-referenced variables are correctly evaluated at t=0 (Phase 2), removing PREVIOUS-specific module handling from LTM compilation (Phase 3), deleting the now-unused `init.stmx` stdlib model (Phase 4), and adding targeted tests plus updating documentation comments (Phase 5).

## Definition of Done

Activate the existing `LoadPrev`/`LoadInitial` opcode infrastructure so that simple `PREVIOUS(x)` and `INIT(x)` calls compile to direct opcodes instead of falling through to stdlib module expansion. This applies to both user model equations and LTM (Loops That Matter) synthetic equations, yielding faster compilation and execution. The stdlib `init` model is deleted (no longer used). General module infrastructure in LTM is preserved for SMOOTH/DELAY/etc. The 2-arg `PREVIOUS(x, init_val)` form continues using module expansion (tracked separately).

Success criteria:
- 1-arg `PREVIOUS(x)` compiles to `LoadPrev` in user models and LTM equations
- `INIT(x)` compiles to `LoadInitial` in user models
- 2-arg `PREVIOUS(x, init_val)` stays on module expansion
- LTM compilation simplified: PREVIOUS-specific module handling removed, general implicit-var support preserved
- Initials runlist extended to include INIT-referenced variables
- `prev_values` not seeded with post-initials state (1-arg PREVIOUS returns 0 at first timestep, matching module behavior)
- stdlib `init.stmx` model and generated code deleted
- stdlib `previous.stmx` model kept (needed for 2-arg PREVIOUS)
- Array/subscript edge cases handled correctly
- All existing tests pass; new tests cover activated opcode paths
- Scaffolding comments updated

## Acceptance Criteria

### prev-init-opcodes.AC1: 1-arg PREVIOUS compiles to LoadPrev
- **prev-init-opcodes.AC1.1 Success:** `PREVIOUS(x)` in a scalar user model equation emits `LoadPrev` opcode (not module expansion)
- **prev-init-opcodes.AC1.2 Success:** `PREVIOUS(x[DimA])` in an arrayed equation emits per-element `LoadPrev` with correct offsets
- **prev-init-opcodes.AC1.3 Failure:** `PREVIOUS(x)` at the first timestep returns 0 (not x's initial value), matching module behavior

### prev-init-opcodes.AC2: INIT compiles to LoadInitial
- **prev-init-opcodes.AC2.1 Success:** `INIT(x)` in a user model equation emits `LoadInitial` opcode
- **prev-init-opcodes.AC2.2 Success:** `INIT(x)` in an aux-only model (no stocks) returns x's initial value correctly (Initials runlist includes x)
- **prev-init-opcodes.AC2.3 Failure:** `initial_values` snapshot is not all zeros when INIT references a variable that has no stock dependency

### prev-init-opcodes.AC3: 2-arg PREVIOUS preserved
- **prev-init-opcodes.AC3.1 Success:** `PREVIOUS(x, init_val)` still uses module expansion and produces identical results to pre-change behavior
- **prev-init-opcodes.AC3.2 Success:** Arrayed 2-arg PREVIOUS creates per-element module instances as before

### prev-init-opcodes.AC4: LTM uses direct opcodes
- **prev-init-opcodes.AC4.1 Success:** LTM link-score equations with `PREVIOUS(y)` compile to `LoadPrev` and produce correct loop scores matching reference data
- **prev-init-opcodes.AC4.2 Success:** PREVIOUS-specific module handling removed from `compile_ltm_equation_fragment()` without affecting SMOOTH/DELAY module support

### prev-init-opcodes.AC5: INIT stdlib model deleted
- **prev-init-opcodes.AC5.1 Success:** `stdlib/init.stmx` deleted, "init" removed from `MODEL_NAMES` and `stdlib_args`, no compilation errors

### prev-init-opcodes.AC6: Cross-cutting
- **prev-init-opcodes.AC6.1 Success:** All existing integration tests pass (previous, builtin_init, LTM test models)
- **prev-init-opcodes.AC6.2 Success:** Scaffolding comments in bytecode.rs, codegen.rs, builtins_visitor.rs updated to reflect activated state

## Glossary

- **PREVIOUS(x)**: A system dynamics builtin function that returns the value of variable `x` from the previous simulation timestep. The 1-arg form returns 0 at the first timestep; the 2-arg form `PREVIOUS(x, init_val)` returns `init_val` instead.
- **INIT(x)**: A builtin function that freezes and returns the value of variable `x` at simulation initialization time (t=0), regardless of when it is evaluated during the run.
- **LoadPrev / LoadInitial**: VM bytecode opcodes that read from snapshot buffers (`prev_values` / `initial_values`) to implement PREVIOUS and INIT without module expansion.
- **Module expansion**: The compilation strategy where each PREVIOUS/INIT call is replaced by instantiating a stdlib submodel (a stock-and-flow module) that tracks the required state.
- **Stdlib models**: Built-in `.stmx` model definitions (e.g., `previous.stmx`, `init.stmx`) that implement stateful builtin functions as stock-and-flow submodels. Generated into Rust source in `stdlib.gen.rs`.
- **Snapshot buffers**: Two per-simulation arrays (`prev_values` and `initial_values`) that store copies of the variable state vector (`curr[]`). `initial_values` is captured after the Initials phase; `prev_values` is captured each timestep after stocks are updated.
- **Initials runlist**: The topologically sorted list of variables evaluated during the initialization phase (t=0). Currently includes stocks, modules, and their transitive dependencies.
- **LTM (Loops That Matter)**: An analysis feature that identifies which feedback loops dominate a model's behavior. It generates synthetic link-score equations that use `PREVIOUS(x)` to compare current and prior timestep values.
- **A2A (array-to-array)**: The compilation context for arrayed (subscripted) variables, where a single equation expands into per-element operations.
- **StepPart**: An enum (`Initials`, `Flows`, `Stocks`) that indicates which simulation phase is currently executing. `LoadPrev` returns 0 during Initials and reads from `prev_values` during other phases.
- **implicit_vars**: Variables synthesized during equation parsing -- typically module instances created by stdlib expansion for functions like PREVIOUS, SMOOTH, and DELAY.

## Architecture

### Current State

`PREVIOUS(x)` and `INIT(x)` have dual implementations: stdlib module expansion (active) and direct VM opcodes (scaffolding, never exercised). The gate at `builtins_visitor.rs:341` forces all calls through module expansion, creating synthetic module instances with stocks and flows that track previous/initial values. The `LoadPrev`/`LoadInitial` opcodes, snapshot buffers (`prev_values`/`initial_values`), and codegen paths exist but are never reached.

### Target State

Simple 1-arg `PREVIOUS(x)` and `INIT(x)` compile directly to `LoadPrev`/`LoadInitial` opcodes. The VM reads from snapshot buffers instead of evaluating module instances. This eliminates module instantiation overhead for the common case and substantially simplifies LTM compilation.

### Gate Logic (builtins_visitor.rs)

Replace the blanket PREVIOUS/INIT gate with an arity-based check:

- **1-arg `PREVIOUS(x)`**: Passes through as `UntypedBuiltinFn` -> lowers to `BuiltinFn::Previous` -> compiles to `LoadPrev { off }`
- **2-arg `PREVIOUS(x, init_val)`**: Falls through to module expansion (stdlib `previous` module)
- **`INIT(x)`**: Passes through as `UntypedBuiltinFn` -> lowers to `BuiltinFn::Init` -> compiles to `LoadInitial { off }`

In A2A (array-to-array) context, when passing through as a builtin, `substitute_dimension_refs` is applied to arguments so dimension references resolve to concrete elements per array position.

### Initials Runlist Extension (db.rs)

Without the INIT module's stock, variables referenced by `INIT(x)` may not appear in the Initials runlist. In `model_dependency_graph_impl()`, walk each variable's AST to detect `BuiltinFn::Init` references. Add their argument variables to the Initials runlist "needed" set alongside stocks and modules. The existing transitive closure pulls in dependencies automatically.

### Snapshot Buffer Fix (vm.rs)

Remove the `prev_values` seeding after the Initials phase. Leave `prev_values` at its initialized-to-zeros state so that `LoadPrev` returns 0 at the first timestep, matching the module expansion behavior for 1-arg PREVIOUS (where `initial_value` defaults to 0). The per-timestep snapshot after stocks correctly maintains `prev_values` from timestep 1 onward. The `initial_values` seeding remains unchanged.

### LTM Simplification (db_ltm.rs)

LTM equations only use 1-arg PREVIOUS (auto-generated link-score formulas). With the arity-based gate, these compile to `LoadPrev` -- no PREVIOUS module instances are created. `LoadPrev` works correctly in LTM fragments because the fragment's dependency slots (filled via module input copies) are captured in the full `prev_values` snapshot.

PREVIOUS-specific code paths are removed from `compile_ltm_equation_fragment()`:
- Searching `parsed.implicit_vars` for PREVIOUS module instances
- Cross-LTM-equation PREVIOUS module dependency resolution
- Adding implicit module vars to mini_metadata
- Merging `implicit_module_refs` into `module_models`

General implicit-var infrastructure for SMOOTH/DELAY stays intact.

### Stdlib Model Cleanup

Delete `stdlib/init.stmx` and its generated code in `stdlib.gen.rs`. Remove "init" from `MODEL_NAMES` and `stdlib_args`. Keep `stdlib/previous.stmx` (needed for 2-arg PREVIOUS module expansion).

## Existing Patterns

The opcode infrastructure follows patterns already established by other VM opcodes. `LoadPrev` and `LoadInitial` mirror the structure of existing load opcodes (`LoadVar`, `LoadModuleInput`) -- single `VariableOffset` parameter, push one value onto the stack. The codegen paths in `codegen.rs:692-737` already emit these opcodes when `BuiltinFn::Previous`/`BuiltinFn::Init` reach them. The VM execution handlers in `vm.rs:1062-1082` correctly branch on `StepPart::Initials` vs other phases.

The arity-based gate approach follows the existing pattern where `is_builtin_fn()` controls which functions pass through as builtins vs. expand to modules. The change integrates naturally: 1-arg PREVIOUS and INIT join the same path as `ABS`, `MAX`, etc.

The Initials runlist extension follows the existing pattern in `model_dependency_graph_impl()` where the "needed" set is filtered by `is_stock || is_module`. Adding `|| init_referenced` is a minimal extension of the same mechanism.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Activate Gate and Fix Snapshot Seeding

**Goal:** Make 1-arg PREVIOUS and INIT compile to direct opcodes and produce correct values.

**Components:**
- `src/simlin-engine/src/builtins_visitor.rs` -- replace blanket PREVIOUS/INIT gate with arity-based check; apply `substitute_dimension_refs` in A2A context for builtins passing through
- `src/simlin-engine/src/vm.rs` -- remove `prev_values` seeding after Initials phase

**Dependencies:** None (first phase)

**Done when:** 1-arg PREVIOUS compiles to `LoadPrev`, INIT compiles to `LoadInitial`, and existing tests pass. Covers `prev-init-opcodes.AC1.1`, `prev-init-opcodes.AC1.2`, `prev-init-opcodes.AC2.1`, `prev-init-opcodes.AC3.1`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Fix Initials Runlist for INIT

**Goal:** Ensure `initial_values` snapshot is correctly populated when INIT references variables not otherwise in the Initials runlist.

**Components:**
- `src/simlin-engine/src/db.rs` -- in `model_dependency_graph_impl()`, detect `BuiltinFn::Init` references in variable ASTs and add referenced variables to the Initials runlist "needed" set

**Dependencies:** Phase 1 (INIT now compiles to LoadInitial)

**Done when:** INIT in aux-only models (no stocks) correctly captures initial values. Covers `prev-init-opcodes.AC2.2`, `prev-init-opcodes.AC2.3`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Simplify LTM Compilation

**Goal:** Remove PREVIOUS-specific module handling from LTM compilation.

**Components:**
- `src/simlin-engine/src/db_ltm.rs` -- remove PREVIOUS module instance searching, cross-equation PREVIOUS dependency resolution, implicit module metadata merging from `compile_ltm_equation_fragment()`; simplify supporting data structures
- `src/simlin-engine/src/db.rs` -- assembly code that iterates LTM implicit vars naturally handles empty collections (no structural changes needed)

**Dependencies:** Phase 1 (PREVIOUS in LTM now compiles to LoadPrev)

**Done when:** LTM tests pass with simplified compilation. Covers `prev-init-opcodes.AC4.1`, `prev-init-opcodes.AC4.2`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Delete INIT Stdlib Model

**Goal:** Remove the unused INIT stdlib module.

**Components:**
- Delete `stdlib/init.stmx`
- `src/simlin-engine/src/stdlib.gen.rs` -- remove INIT model definition and "init" from `MODEL_NAMES`
- `src/simlin-engine/src/builtins_visitor.rs` -- remove "init" from `stdlib_args()`

**Dependencies:** Phase 2 (INIT no longer uses module expansion)

**Done when:** No references to stdlib `init` model remain. All tests pass. Covers `prev-init-opcodes.AC5.1`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: New Tests and Comment Cleanup

**Goal:** Add test coverage for activated opcode paths and update documentation.

**Components:**
- `src/simlin-engine/tests/` or `src/simlin-engine/src/db_tests.rs` -- new tests for LoadPrev/LoadInitial opcode emission, arrayed PREVIOUS/INIT, INIT in aux-only model
- `src/simlin-engine/src/bytecode.rs` -- update opcode documentation (remove "scaffolding" language)
- `src/simlin-engine/src/compiler/codegen.rs` -- update comments about when LoadPrev/LoadInitial are reached
- `src/simlin-engine/src/builtins_visitor.rs` -- update gate comments
- Rename existing parity tests to reflect they now exercise the opcode path

**Dependencies:** Phases 1-4

**Done when:** New tests pass, comments reflect activated state. Covers `prev-init-opcodes.AC1.3`, `prev-init-opcodes.AC3.2`, `prev-init-opcodes.AC6.1`, `prev-init-opcodes.AC6.2`.
<!-- END_PHASE_5 -->

## Additional Considerations

**Semantic difference at first timestep:** 1-arg `PREVIOUS(x)` returns 0 at the first timestep (matching module behavior where `initial_value` defaults to 0). This differs from the aspirational comment in the scaffolding code that suggested returning `x(0)`. The 0-return is correct for compatibility and avoids division-by-zero in LTM link-score equations.

**2-arg PREVIOUS opcode support:** Extending `LoadPrev` to handle the 2-arg form (`PREVIOUS(x, init_val)`) would require a new opcode pattern with conditional branching based on `StepPart`. This is tracked separately and not part of this design. Once implemented, the PREVIOUS stdlib model could also be deleted.
