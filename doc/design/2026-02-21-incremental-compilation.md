# Incremental Compilation Design

## Summary

Simlin's simulation engine currently recompiles the entire model from scratch on
every edit -- parsing all equations, recomputing all dependencies, and
regenerating all bytecode even when only a single variable changed. This design
replaces that monolithic pipeline with an incremental one built on salsa, a
framework for demand-driven incremental computation. The core idea is to
decompose each compilation stage (parsing, lowering, dependency analysis,
bytecode generation) into fine-grained, per-variable tracked functions whose
results salsa automatically caches and selectively invalidates. When a user
edits one equation in a 100-variable model, only that variable's compilation
stages re-execute; everything else is served from cache.

A key technical challenge is that the current bytecode uses raw integer offsets,
which shift whenever a variable is added or removed -- invalidating every cached
fragment. The design solves this with "symbolic bytecode": per-variable
compilation emits opcodes that reference variables by identity rather than by
offset, and a cheap final assembly pass resolves these to concrete integers
using the current layout. This decouples individual variable compilation from
the model's global structure, maximizing cache reuse. The same incremental
approach extends to the Loops That Matter (LTM) analysis pipeline, where causal
graph construction and loop scoring equations are decomposed into tracked
functions that only recompute when their actual inputs (dependency sets, not
mere equation text) change. On the FFI boundary, the salsa database persists
inside the existing `SimlinProject` struct across calls, eliminating the current
double-compilation where `apply_patch` and `sim_new` each independently compile
the model.

## Definition of Done

1. **Incremental compilation via salsa**: simlin-engine's compilation pipeline (datamodel -> ModelStage0 -> ModelStage1 -> dependency analysis -> compiler::Module -> bytecode) uses salsa tracked functions and tracked structs so that model edits (equation changes, constant changes, variable add/remove, dimension changes, module connection changes) only recompute affected portions of the pipeline.

2. **LTM integration**: Both LTM modes (exhaustive and discovery) have their synthetic variable generation incrementalized as salsa tracked functions -- causal graph construction, loop detection, and link/loop/relative score variable generation only recompute when their actual inputs change. The post-simulation `discover_loops()` algorithm remains as-is.

3. **libsimlin integration**: The salsa database persists inside `SimlinProject` across FFI calls, so `apply_patch` and `sim_new` share cached compilation state. The FFI signatures remain unchanged -- this is invisible to TypeScript, Python, and other consumers.

4. **Correctness preserved**: All existing simulation tests (`tests/simulate*.rs`) and LTM integration tests continue to pass with identical numerical results.

5. **Performance**: All edit operations cause proportionally less compilation work than a full rebuild.

## Acceptance Criteria

### incremental-compilation.AC1: Incremental compilation via salsa
- **incremental-compilation.AC1.1 Success:** Changing a variable's equation text (same dependencies) only reparses, relowers, and recompiles that variable's fragment. No other variables' fragments recompute.
- **incremental-compilation.AC1.2 Success:** Changing a variable's equation text (different dependencies) triggers dependency graph and runlist recomputation for the affected model, plus recompilation of the changed variable's fragment.
- **incremental-compilation.AC1.3 Success:** Adding a variable triggers layout recomputation and assembly, but all existing variables' cached fragments are reused.
- **incremental-compilation.AC1.4 Success:** Removing a variable triggers layout recomputation and assembly, but remaining variables' cached fragments are reused.
- **incremental-compilation.AC1.5 Success:** Changing a dimension definition recompiles only variables using that dimension.
- **incremental-compilation.AC1.6 Success:** Changing module connections triggers dependency graph updates for the affected model only.

### incremental-compilation.AC2: LTM integration
- **incremental-compilation.AC2.1 Success:** Equation edit with unchanged dependency set does not trigger causal graph reconstruction, loop detection, or cycle partition recomputation.
- **incremental-compilation.AC2.2 Success:** Equation edit with changed dependency set triggers causal graph and loop detection recomputation, but only link score equations for affected links regenerate.
- **incremental-compilation.AC2.3 Success:** Link score equation for a link targeting variable Z only recomputes when Z's equation text or dependency set changes.
- **incremental-compilation.AC2.4 Success:** Stdlib dynamic module composite scores (SMOOTH, DELAY, TREND) compute once and are never recomputed.
- **incremental-compilation.AC2.5 Success:** Discovery mode all-links generation follows the same per-link incrementality as exhaustive mode.
- **incremental-compilation.AC2.6 Success:** Post-simulation discover_loops() algorithm is unchanged and produces identical results.

### incremental-compilation.AC3: libsimlin integration
- **incremental-compilation.AC3.1 Success:** apply_patch followed by sim_new triggers only one compilation pass (not two).
- **incremental-compilation.AC3.2 Success:** FFI function signatures are unchanged. Existing TypeScript, Python, and C callers work without modification.
- **incremental-compilation.AC3.3 Success:** Multiple sequential patches each trigger only incremental recomputation of affected portions.
- **incremental-compilation.AC3.4 Success:** Running simulations are isolated from subsequent patches (snapshot semantics).

### incremental-compilation.AC4: Correctness preserved
- **incremental-compilation.AC4.1 Success:** All tests in tests/simulate*.rs pass with identical numerical results.
- **incremental-compilation.AC4.2 Success:** All LTM integration tests in tests/simulate_ltm.rs pass with identical results.
- **incremental-compilation.AC4.3 Success:** Incrementally compiled bytecode produces identical output to full recompilation for the same model state.

### incremental-compilation.AC5: Performance
- **incremental-compilation.AC5.1 Success:** Single equation edit on a 100-variable model completes in less time than full recompilation (measurable via benchmark).
- **incremental-compilation.AC5.2 Success:** Variable add/remove on a 100-variable model completes in less time than full recompilation.

## Glossary

- **salsa**: A Rust framework for demand-driven incremental computation. Functions are declared as "tracked," and salsa automatically memoizes their results, traces which inputs each function reads, and invalidates only the functions whose inputs actually changed. Originally developed for rust-analyzer.
- **tracked function**: A salsa-managed function whose return value is memoized. Salsa records which inputs the function reads during execution and only re-executes it when those specific inputs change.
- **tracked struct**: A salsa-managed struct whose fields are individually tracked for change detection. When a field is updated, only tracked functions that read that specific field are invalidated.
- **`#[salsa::input]`**: A salsa annotation marking a struct as an external input to the computation graph. Input values are set imperatively by the application; salsa traces which tracked functions depend on each input.
- **`#[salsa::interned]`**: A salsa annotation that maps a value (like a variable name string) to a compact integer ID. Interned values get cheap copy and equality comparison, avoiding repeated string cloning and hashing.
- **`#[salsa::accumulator]`**: A salsa mechanism for collecting side-channel data (like compilation errors) during tracked function execution without affecting memoization or invalidation of the function's return value.
- **LTM (Loops That Matter)**: A system dynamics analysis technique that quantifies the relative dominance of feedback loops during simulation. It instruments a model with synthetic "score" variables that measure each loop's contribution to system behavior at each timestep.
- **symbolic bytecode**: The intermediate representation introduced by this design where per-variable compiled opcodes reference variables by `VariableId` (an interned identifier) rather than by integer memory offsets. This makes fragments independent of the model's variable layout.
- **`CompiledVarFragment`**: A tracked struct holding symbolic bytecode for a single variable. Because it uses symbolic references, it remains valid even when other variables are added or removed.
- **assembly (in this context)**: The final compilation pass that resolves symbolic variable references in `CompiledVarFragment`s to concrete integer offsets using the current layout, producing executable bytecode for the VM.
- **runlist**: A topologically sorted execution order for variables within a model. The VM evaluates variables in runlist order to ensure each variable's dependencies are computed before it is. Separate runlists exist for initial values, flows, and stocks.
- **causal graph**: A directed graph where nodes are model variables and edges represent causal dependencies. Used by LTM to find feedback loops.
- **link score equation**: A synthetic equation generated by LTM that quantifies the strength of influence along a single causal link at each simulation timestep.
- **ceteris paribus**: Latin for "all other things being equal." In LTM link scoring, this refers to the formula that measures a link's influence by varying only the source variable while holding all other inputs to the target at their previous values.
- **cycle partition**: A grouping of stocks into strongly connected components in the stock-to-stock reachability graph. Relative loop scores are only meaningful within a single partition.
- **Johnson's algorithm**: A graph algorithm for finding all elementary circuits (simple cycles) in a directed graph. Used by LTM to enumerate all feedback loops in the causal graph.
- **`SimFloat` / generic `F`**: The current trait and generic type parameter that abstracts over `f64` and `f32` throughout the engine. This design removes it, hardcoding `f64`, because salsa tracked functions work poorly with generic type parameters.
- **`SourceVariable` / `SourceModel` / `SourceProject`**: The salsa input types that decompose the monolithic `datamodel::Project` into fine-grained pieces. Each `SourceVariable` holds one variable's equation, kind, and metadata.
- **`SimlinDb`**: The salsa database struct that holds all compilation state. It lives inside `SimlinProject` and persists across FFI calls.
- **`datamodel::Project`**: The canonical serializable representation of a Simlin model. It remains the source of truth for persistence; the salsa database is a derived compilation cache.
- **`ModelStage0` / `ModelStage1`**: The current monolithic compilation stages. Stage0 parses all equations; Stage1 resolves modules and lowers the AST. This design replaces them with per-variable tracked functions.
- **stdlib dynamic modules**: Built-in model definitions (SMOOTH, DELAY, TREND) that implement stateful functions as sub-models. Their internal causal graphs are static, so LTM scores for them compute once.

## Architecture

### Approach: Full Pipeline Salsa with Symbolic Bytecode

The entire compilation pipeline is restructured as salsa tracked functions with
per-variable granularity. A key innovation is **symbolic bytecode**: per-variable
compilation produces opcodes that reference `VariableId` instead of raw integer
offsets. A cheap assembly pass resolves symbols to concrete offsets using the
current layout. This decouples per-variable bytecode caching from layout
stability, so adding/removing variables doesn't invalidate cached bytecode
fragments.

### Salsa Database

`SimlinDb` is defined in simlin-engine and holds `salsa::Storage<Self>`. It owns
all compilation state. libsimlin's `SimlinProject` holds a `SimlinDb` alongside
the existing `datamodel::Project`. The datamodel remains the canonical
serializable representation; the db is a compilation cache derived from it.

### Input Decomposition

Rather than a single `datamodel::Project` input (which would invalidate
everything on any change), inputs are decomposed:

- **`SourceProject`** (`#[salsa::input]`): Top-level container with model names,
  sim specs, and dimension definitions.
- **`SourceModel`** (`#[salsa::input]`): Per-model input with model name and
  variable name list.
- **`SourceVariable`** (`#[salsa::input]`): Per-variable input with equation
  text, variable kind (stock/flow/aux/module), units, graphical function data,
  and dimension subscripts.

When `apply_patch` modifies a single variable's equation, only that variable's
`SourceVariable` input changes. Salsa traces which tracked functions read that
input and only invalidates those.

### Interned Identifiers

`#[salsa::interned]` types `VariableId` and `ModelId` replace the current
`Ident<Canonical>` (which is String-backed and cloned frequently). Interned
values get cheap integer-based copy and comparison.

### Tracked Pipeline

Each compilation stage becomes a tracked function:

```
SourceVariable (input)
    |
    v
parse_variable(db, model, var) -> ParsedVariable          [per-variable]
    |
    v
lower_variable(db, model, var) -> LoweredVariable          [per-variable]
    |
    v
variable_dependencies(db, model, var) -> DependencySet      [per-variable]
    |
    v
model_dependency_graph(db, model) -> (DepMap, Runlists)     [per-model]
    |
    v
compile_variable(db, model, var) -> CompiledVarFragment     [per-variable]
    |                                (symbolic opcodes)
    v
compute_layout(db, model) -> VariableLayout                 [per-model]
    |
    v
assemble_module(db, model, inputs) -> CompiledModule        [per-instance]
    |                                  (resolved opcodes)
    v
assemble_simulation(db) -> CompiledSimulation               [project-wide]
```

### Symbolic Bytecode

Per-variable compilation produces `CompiledVarFragment` containing symbolic
opcodes that reference `VariableId` instead of raw integer offsets. This
fragment depends only on the variable's lowered AST and known variable set,
NOT on the layout.

`compute_layout` is a separate tracked function mapping `VariableId ->
(offset, size)`. When a variable is added/removed, only this function reruns.

`assemble_module` takes fragments + layout and resolves symbolic references to
concrete offsets. This is O(total_opcodes) substitution. It also assigns
graphical function IDs, module declaration IDs, and dimension list IDs using
the same symbolic-reference-then-resolve pattern.

**Edit impact by type:**

| Edit type | What reruns |
|-----------|-------------|
| Equation edit (same deps) | Fragment for that variable. Assembly (cheap). |
| Equation edit (new deps) | Fragment, dep graph, runlists. Assembly. |
| Variable add/remove | Layout. Assembly. All fragments cached. |
| Dimension change | Affected fragments, layout. Assembly. |

### LTM Integration

LTM analysis chains off the dependency analysis stage:

```
variable_dependencies (per-variable, already tracked)
    |
    v
causal_graph(db, model) -> CausalGraph                     [per-model]
    |
    v
detect_loops(db, model) -> Vec<Loop>                        [per-model]
    |
    v
compute_cycle_partitions(db, model) -> CyclePartitions      [per-model]
    |
    v
link_score_equation(db, model, from, to) -> Equation        [per-link]
    |
    v
loop_score_equation(db, loop_id) -> Equation                [per-loop]
    |
    v
relative_loop_score_equation(db, loop_id) -> Equation       [per-loop]
```

The causal graph depends on variable dependency sets, NOT equation text. If a
variable's equation changes from `a + b` to `a * b`, the dependency set
`{a, b}` is unchanged, so the causal graph, loops, and most synthetic variables
are cached.

Per-link score equations depend on the *target variable's equation text* (for
ceteris-paribus substitution) and its dependency set. When a variable's equation
changes, only link score equations where that variable is the *target*
recompute.

Module composite scores for stdlib dynamic modules (SMOOTH, DELAY, TREND) are
tracked per-module-model. Since stdlib internal graphs are static, these compute
once and never recompute.

LTM synthetic variables feed into the same per-variable compilation pipeline as
regular variables. They get their own `SourceVariable` inputs set
programmatically by the LTM tracked functions.

Discovery mode uses `all_links` (tracked, depends on causal graph) to generate
link scores for every causal edge. Same per-link tracking applies. The
post-simulation `discover_loops()` is unchanged.

### Error Handling

Errors move from struct fields (`ModelStage1.errors`, `Variable.errors`) to a
`#[salsa::accumulator]`:

```rust
#[salsa::accumulator]
pub struct CompilationDiagnostic {
    pub model: ModelId,
    pub variable: Option<VariableId>,
    pub error: Error,
}
```

Tracked functions `accumulate()` errors as a side channel. Callers collect via
`compile::accumulated::<CompilationDiagnostic>(db)`. Errors don't affect whether
downstream queries need recomputation.

### libsimlin Integration

`SimlinProject` holds a `SimlinDb` alongside the `datamodel::Project`:

- **`apply_patch`**: Modifies the datamodel, then syncs affected
  `SourceVariable`/`SourceModel` inputs on the db. Salsa incrementally
  recomputes invalidated functions. Diagnostics collected via accumulators.
- **`sim_new`**: Reads the already-compiled `CompiledSimulation` from the db
  (cache hit if no changes since last patch). Takes a snapshot of the bytecode
  for the VM to own. Subsequent edits don't affect running simulations.

This eliminates the current double compilation (patch validation + simulation
creation).

### Removing the `F: FloatLike` Generic

The `SimFloat` trait and its generic parameter `F` are removed. All compilation
and simulation uses `f64` directly. This eliminates generic parameter threading
through every stage and avoids the need for `unsafe(non_update_types)` on salsa
tracked functions. Historical f32 golden test data is updated where needed.

## Existing Patterns

### Current Pipeline Structure

The current pipeline uses a multi-stage transformation:
`datamodel::Project` -> `ModelStage0` (parse equations) -> `ModelStage1`
(resolve modules, lower AST) -> `set_dependencies` (transitive deps,
runlists) -> `Module<F>` (assign offsets, lower expressions) ->
`CompiledModule<F>` (emit bytecode) -> `CompiledSimulation<F>` (assemble).

Each stage is a monolithic per-model operation. `ModelStage0::new` parses all
variables at once; `ModelStage1::new` lowers all at once; `Module::new` compiles
all variables in the model.

### Variable Type System

Variables use a generic enum `Variable<M, E>` that specializes across stages:
`Variable<datamodel::ModuleReference, Expr0>` in Stage0,
`Variable<ModuleInput, Expr2>` in Stage1. The new design preserves this
pattern but wraps each stage's output in salsa tracked structs.

### Offset Assignment

`build_metadata()` in `compiler/mod.rs` assigns sequential offsets in
alphabetical variable order. The root model reserves offsets 0-3 for implicit
variables (time, dt, initial_time, final_time). This sequential scheme is
replaced by the symbolic bytecode approach, though the final resolved offsets
can use the same alphabetical ordering for determinism.

### Bytecode Architecture

Opcodes like `LoadVar { off }`, `AssignCurr { off }` use module-relative
integer offsets. At runtime, the VM adds `module_off` to get absolute positions
in the `curr[]`/`next[]` arrays. The symbolic bytecode layer sits above this:
symbolic opcodes use `VariableId`, and assembly resolves to the existing
integer-offset opcodes.

### LTM Synthetic Variable Injection

`inject_ltm_vars()` patches the `datamodel::Project` by appending synthetic
variables to each model's variable list. The patched datamodel is then compiled
through the normal pipeline via `Project::from()`. The new design follows the
same pattern: LTM tracked functions produce synthetic `SourceVariable` inputs
that enter the same compilation pipeline.

### Error Storage

Errors are currently stored as fields on `ModelStage1` and `Variable`. This
mixes error state with computed state. The new design diverges by using salsa
accumulators, which separate error collection from computation results.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Remove `F: SimFloat` Generic

**Goal:** Hardcode `f64` throughout simlin-engine, eliminating the generic
parameter that complicates salsa integration.

**Components:**
- `SimFloat` trait in `src/simlin-engine/src/float.rs` -- remove trait, replace
  all `F: SimFloat` with `f64`
- `Module<F>`, `Var<F>`, `Compiler<F>` in `src/simlin-engine/src/compiler/` --
  remove generic parameter
- `CompiledSimulation<F>`, `Vm<F>`, `Results<F>` in
  `src/simlin-engine/src/vm.rs`, `src/simlin-engine/src/results.rs` -- remove
  generic parameter
- `ByteCode<F>`, `Opcode` variants in `src/simlin-engine/src/bytecode.rs` --
  remove generic parameter
- `compile_project<F>` in `src/simlin-engine/src/interpreter.rs` -- remove
  generic parameter
- Update golden test data in `test/` if f32 results diverge from f64

**Dependencies:** None (first phase)

**Done when:** All tests pass with f64. No `SimFloat` trait or generic `F`
parameter remains in the compilation/simulation pipeline.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Introduce Salsa Database and Interned Identifiers

**Goal:** Add salsa as a dependency, define the database and interned types,
and wire the db into libsimlin without changing compilation behavior.

**Components:**
- `salsa` dependency in `src/simlin-engine/Cargo.toml` (from crates.io, not
  third_party/)
- `SimlinDb` struct in `src/simlin-engine/src/db.rs` (new) -- database
  definition with `salsa::Storage<Self>`
- `VariableId`, `ModelId` interned types in `src/simlin-engine/src/db.rs`
- `SimlinProject` in `src/libsimlin/src/lib.rs` -- add `SimlinDb` field
  alongside existing `Project`
- Input types `SourceProject`, `SourceModel`, `SourceVariable` in
  `src/simlin-engine/src/db.rs`
- Sync function that populates salsa inputs from `datamodel::Project`

**Dependencies:** Phase 1

**Done when:** `SimlinDb` compiles, interned identifiers work, libsimlin holds
a db instance. Compilation still uses the old pipeline; the db is populated but
not yet read.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Per-Variable Parsing and Lowering

**Goal:** Decompose `ModelStage0::new` and `ModelStage1::new` into per-variable
salsa tracked functions so equation edits only reparse/relower the affected
variable.

**Components:**
- `parse_variable` tracked function in `src/simlin-engine/src/model.rs` or new
  module -- takes `SourceVariable`, returns parsed `Expr0` AST wrapped in a
  tracked struct
- `lower_variable` tracked function -- takes parsed variable, dimension
  context, module definitions; returns lowered `Expr2` AST
- Tracked struct wrappers (`ParsedVariable`, `LoweredVariable`) satisfying
  salsa's `Clone + Eq + Hash + Update` requirements
- Orchestrator functions that call per-variable tracked functions for all
  variables in a model, replacing the current monolithic constructors
- `CompilationDiagnostic` accumulator for parse/lower errors

**Dependencies:** Phase 2

**Done when:** Parsing and lowering use salsa tracked functions. Changing one
variable's equation only reparses/relowers that variable (verifiable via salsa
event logging). All tests pass.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Dependency Analysis and Runlists

**Goal:** Extract dependency analysis into tracked functions so equation edits
that don't change dependencies skip dependency recomputation.

**Components:**
- `variable_dependencies` tracked function in `src/simlin-engine/src/model.rs`
  -- per-variable, extracts direct `DependencySet` from `LoweredVariable`
- `model_dependency_graph` tracked function -- per-model, aggregates
  per-variable dependency sets, computes transitive dependencies, produces
  topologically sorted runlists
- Replaces current `set_dependencies()` method on `ModelStage1` and the
  `all_deps()` recursive function
- Circular dependency detection as a separate validation pass using
  accumulators

**Dependencies:** Phase 3

**Done when:** Dependency analysis uses salsa tracked functions. Changing an
equation from `a + b` to `a * b` (same deps) does not trigger dependency
recomputation (verifiable via salsa event logging). All tests pass.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Symbolic Bytecode and Layout Separation

**Goal:** Introduce symbolic opcodes and late offset resolution so per-variable
bytecode fragments are layout-independent.

**Components:**
- `SymbolicOpcode` enum in `src/simlin-engine/src/compiler/` -- mirrors
  `Opcode` but uses `VariableId` instead of integer offsets for variable
  references, symbolic IDs for graphical functions, module declarations, and
  dimension lists
- `CompiledVarFragment` tracked struct -- per-variable compilation result
  containing `Vec<SymbolicOpcode>` and metadata (referenced graphical
  functions, modules)
- `compile_variable` tracked function -- takes `LoweredVariable` and produces
  `CompiledVarFragment`. Replaces `Var::new` + per-variable codegen
- `compute_layout` tracked function -- per-model, maps `VariableId ->
  (offset, size)`. Depends on the set of variable names and their sizes, NOT
  on equations
- `assemble_module` tracked function -- per-module-instance, takes fragments +
  layout + runlists, resolves symbolic references to concrete offsets,
  concatenates into `CompiledModule`
- `assemble_simulation` tracked function -- project-wide, assembles all modules
  into `CompiledSimulation`

**Dependencies:** Phase 4

**Done when:** Per-variable bytecode fragments are layout-independent. Adding a
variable does not invalidate cached fragments (verifiable via salsa event
logging). Assembly produces identical bytecode to the old pipeline. All tests
pass.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: LTM Integration

**Goal:** Decompose LTM analysis into tracked functions that chain off the
dependency analysis pipeline.

**Components:**
- `causal_graph` tracked function in `src/simlin-engine/src/ltm.rs` --
  per-model, builds `CausalGraph` from variable dependency sets. Replaces
  `CausalGraph::from_model()`
- `detect_loops` tracked function -- per-model, runs Johnson's algorithm on
  causal graph. Replaces loop detection in `detect_loops()`
- `compute_cycle_partitions` tracked function -- per-model, computes
  stock-to-stock SCCs
- `link_score_equation` tracked function in
  `src/simlin-engine/src/ltm_augment.rs` -- per-link, generates ceteris-paribus
  equation. Depends on target variable's equation and dependency set
- `loop_score_equation`, `relative_loop_score_equation` tracked functions --
  per-loop
- `module_composite_scores` tracked function -- per-stdlib-dynamic-module,
  generates internal link scores, pathway scores, composite scores. Computes
  once since stdlib graphs are static
- LTM synthetic variables injected as `SourceVariable` inputs, compiled through
  the same per-variable pipeline from Phase 5
- Discovery mode: `all_links` tracked function generates link scores for every
  causal edge; `discover_loops()` post-simulation algorithm unchanged

**Dependencies:** Phase 5

**Done when:** LTM analysis uses salsa tracked functions. Changing a variable's
equation without changing its dependencies does not trigger loop redetection
(verifiable via salsa event logging). All LTM integration tests pass
(`tests/simulate_ltm.rs`).
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Eliminate Double Compilation in libsimlin

**Goal:** Make `apply_patch` and `sim_new` share the salsa database so
compilation results are reused across FFI calls.

**Components:**
- `SimlinProject` in `src/libsimlin/src/lib.rs` -- restructure to hold
  `SimlinDb` as primary compilation state
- `apply_patch` in `src/libsimlin/src/patch.rs` -- after modifying the
  datamodel, sync affected `SourceVariable`/`SourceModel` inputs on the db.
  Collect diagnostics via `compile::accumulated::<CompilationDiagnostic>(db)`
- `simlin_sim_new` in `src/libsimlin/src/simulation.rs` -- read
  `CompiledSimulation` from db (cache hit if no changes since last patch).
  Snapshot bytecode for VM ownership
- Remove the current `Project` struct from `SimlinProject` (the db subsumes
  its role)

**Dependencies:** Phase 6

**Done when:** A patch followed by `sim_new` triggers only one compilation (not
two). Verifiable by logging salsa recomputations during the patch+sim sequence.
All libsimlin tests and integration tests pass.
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Error Accumulator Migration

**Goal:** Move all compilation errors from struct fields to salsa accumulators.

**Components:**
- Remove `errors` field from model and variable structs across
  `src/simlin-engine/src/model.rs`
- Remove `unit_errors` and `unit_warnings` fields
- All error-producing tracked functions (parsing, lowering, dependency
  analysis, compilation) use `CompilationDiagnostic.accumulate(db)` instead of
  storing errors on return values
- `gather_error_details` in `src/libsimlin/src/patch.rs` collects errors via
  accumulated diagnostics instead of walking struct fields
- Error types (`EquationError`, `Error`, `UnitError`) gain `Clone + Eq + Hash`
  derives as needed for accumulator compatibility

**Dependencies:** Phase 7

**Done when:** No compilation errors stored as struct fields. All errors
collected via salsa accumulators. Error reporting in libsimlin produces
identical results. All tests pass.
<!-- END_PHASE_8 -->

## Additional Considerations

### Salsa Data Type Constraints

Salsa tracked struct fields must implement `Clone + Eq + Hash + Update`. The
existing AST types (`Expr0`, `Expr2`) and `Variable` enum will need these
derives. `salsa::Update` is implemented for standard library types (`Vec`,
`HashMap`, `BTreeMap`, `Option`, `Result`, `String`, `Arc`, `Box`), so
most compositions work automatically. Types containing raw pointers or interior
mutability will need adjustment.

### Simulation Isolation

When `sim_new` reads the `CompiledSimulation` from the db, it takes a clone of
the bytecode data. The VM owns this snapshot. Subsequent edits to the db
(new patches) don't affect running simulations, preserving the current isolation
guarantee.

### Determinism

The design preserves deterministic compilation output. Variable layouts use the
same alphabetical ordering as the current `build_metadata()`. Loop IDs use the
same deterministic assignment from `assign_deterministic_loop_ids()`. Salsa's
memoization is transparent -- cached results are identical to recomputed results.
