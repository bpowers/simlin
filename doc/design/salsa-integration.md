# Salsa Integration: Refactoring Recommendations

## Executive Summary

This document analyzes the simlin-engine compilation pipeline and recommends
preparatory refactoring to adopt [salsa](https://github.com/salsa-rs/salsa) for
incremental compilation. The primary motivation is long-lived browser and
Jupyter sessions where models are tweaked incrementally -- either by human edits
or programmatic optimization loops. For large models, avoiding full
recompilation on every edit reduces latency and resource use.

## How Salsa Works

Salsa is a framework for on-demand, incremental computation. The core concepts:

- **Inputs** (`#[salsa::input]`): mutable root data that the user sets. When an
  input changes, salsa knows which downstream computations are potentially
  invalidated.
- **Tracked functions** (`#[salsa::tracked]`): pure functions whose results are
  memoized. Salsa re-executes them only when their inputs (transitively) change.
  These are the "queries" of the system.
- **Tracked structs** (`#[salsa::tracked]`): intermediate data created by
  tracked functions. Salsa tracks which fields are read and only invalidates
  downstream queries when the *actually-read* fields change.
- **Interned values** (`#[salsa::interned]`): deduplicated values (like
  identifiers) that get a cheap, hashable ID.
- **Accumulators** (`#[salsa::accumulator]`): side-channel output (like
  diagnostics/errors) that are collected during query execution without
  threading through return types.
- **Database** (`#[salsa::db]`): the container that owns all salsa storage. All
  tracked functions take `&dyn Db` as their first argument.

Key constraints salsa places on data:
- Data stored in tracked structs must implement `Clone`, `Eq`, `Hash`, and
  `salsa::Update`.
- Tracked functions must be **pure** -- no side effects, no interior mutability.
- All data flows through the database; you can't stash pointers to tracked data
  outside the db.
- The `'db` lifetime ties tracked struct references to the database revision.

## Current Compilation Pipeline

The engine has a multi-stage pipeline, each stage corresponding to a set of
transformations:

```
datamodel::Project                          [user input / protobuf]
    │
    ▼
ModelStage0                                 [parse equations → Expr0 AST]
    │  (resolve module inputs, lower AST)
    ▼
ModelStage1                                 [resolved AST → Expr2, variable deps]
    │  (set_dependencies: topo-sort, build runlists per instantiation)
    ▼
compiler::Module<F>                         [lower Expr2 → Expr<F>, assign offsets]
    │  (codegen: emit bytecode opcodes)
    ▼
bytecode::CompiledModule<F>                 [bytecode + lookup tables]
    │  (assemble all modules)
    ▼
vm::CompiledSimulation<F>                   [ready to execute]
    │
    ▼
vm::Vm<F>  →  Results                      [simulation execution]
```

### Stage Details

1. **`datamodel::Project → ModelStage0`** (`model.rs:ModelStage0::new`)
   - Parses equation text into `Expr0` AST nodes
   - Creates `Variable<ModuleReference, Expr0>` instances
   - Per-model, no cross-model dependencies

2. **`ModelStage0 → ModelStage1`** (`model.rs:ModelStage1::new`)
   - Resolves module inputs (maps `datamodel::ModuleReference` → `ModuleInput`)
   - Lowers `Expr0` → `Expr2` (resolves dimension indexes)
   - Per-model, depends on the `ModelStage0` map for module input resolution

3. **`ModelStage1::set_dependencies`** (`model.rs`)
   - Computes `all_deps()` -- transitive dependency graph per variable
   - Builds topologically sorted runlists (initials, flows, stocks)
   - Creates `ModuleStage2` per (model, input_set) combination
   - Depends on *all* `ModelStage1` instances (for cross-module dataflow)

4. **`compiler::Module<F>::new`** (`compiler/mod.rs`)
   - Assigns variable offsets (`build_metadata`)
   - Lowers `Expr2` → `compiler::Expr<F>` via `Var::new`
   - Extracts temp array sizes
   - Produces runlists of `Expr<F>` (flat expression lists)

5. **`compiler::Module<F>::compile`** → `CompiledModule<F>` (`compiler/codegen.rs`)
   - Walks `Expr<F>` trees and emits `Opcode` bytecode
   - Builds `ByteCodeContext` with lookup tables, dimensions, module declarations

6. **`compile_project`** (`interpreter.rs:1610`)
   - Enumerates all module instantiations
   - Calls `Module::new` + `module.compile()` for each
   - Assembles `CompiledSimulation` with flattened offsets

### Key Observations

**What changes when a user edits a single variable's equation:**
- The `datamodel::Variable` changes
- Its `ModelStage0` variable changes (re-parse)
- Its `ModelStage1` variable changes (re-lower AST)
- Its dependencies *might* change → runlists might change
- Its `compiler::Expr<F>` changes → its bytecode changes
- **But**: other variables' bytecode does NOT need to change (assuming offsets
  are stable)

**What changes when a user adds/removes a variable:**
- Offsets for all subsequent variables shift
- All runlists may change
- All bytecode needs regeneration (offset-dependent)

**What changes when a user edits a dimension definition:**
- All variables using that dimension need recompilation
- Array sizes change → offsets shift

## Recommended Refactoring (Pre-Salsa)

The goal is to restructure the compilation pipeline so that each stage is a
**pure function of well-defined inputs**, making it natural to wrap with
`#[salsa::tracked]`. The refactoring is ordered from least to most invasive.

### 1. Decouple Variable Offsets from Compilation Order

**Problem**: Variable offsets are assigned by iterating `model.variables` in
sorted key order and assigning sequential slots. This means *any* variable
addition/removal invalidates *all* offsets, which in turn invalidates *all*
bytecode.

**Recommendation**: Separate offset calculation into its own tracked query that
produces a `HashMap<Ident, (offset, size)>`. This is already partially done
(`build_metadata`), but it's called inside `Module::new`. Extract it so salsa
can memoize it independently.

**Incremental benefit**: If a variable's equation changes but the set of
variables doesn't, offsets stay stable → bytecode for unmodified variables can
be reused.

**Concrete steps**:
- Move `build_metadata` and `calc_n_slots` to standalone functions (they
  already almost are, just need the `Project` parameter decoupled)
- Make offset computation depend only on the *set* of variable names + their
  sizes, not on their equations
- Introduce a `VariableLayout` struct that captures (name → offset, n_slots)
  as a salsa-tracked result

### 2. Make Per-Variable Compilation Independent

**Problem**: `compiler::Module::new` compiles *all* variables in a model at
once, iterating runlists and calling `Var::new` for each. This means changing
one variable recompiles all of them.

**Recommendation**: Make `Var::new` (which lowers a single `Variable` to
`Vec<Expr<F>>`) a standalone tracked function keyed on `(model_name,
var_ident)`. Its inputs are:
- The `Variable`'s AST
- The offset map (from step 1)
- The module-models map (for module variable resolution)
- The dimension context
- Whether this is an initial or flow/stock computation

**Incremental benefit**: When variable `foo`'s equation changes, only `foo`'s
`Expr<F>` is recomputed. Other variables' expressions are memoized.

**Concrete steps**:
- Factor `Var::new` to take explicit parameters rather than a `Context` that
  bundles everything. The `Context` struct aggregates too many concerns.
- Move the `Context` construction out of `Module::new` so each `Var::new` call
  can be independently memoized.

### 3. Split Dependency Analysis from Runlist Construction

**Problem**: `ModelStage1::set_dependencies` does three things in one pass:
1. Computes `all_deps()` (transitive dependencies per variable)
2. Builds topologically sorted runlists
3. Stores errors on variables

This makes it hard to cache any of these independently.

**Recommendation**: Split into separate functions:
- `compute_variable_deps(model, var_ident, is_initial) → DependencySet`
  (per-variable, could be a tracked function)
- `build_dependency_map(model, input_set, is_initial) → DependencyMap`
  (aggregates per-variable deps)
- `build_runlists(model, dep_map, step_part) → Vec<Ident>`
  (topological sort, depends only on the dep map)

**Incremental benefit**: If variable `foo`'s equation changes but its
dependency set stays the same (e.g., `a + b` → `a * b`), runlists don't need
recomputation, and bytecode for other variables is definitely stable.

**Concrete steps**:
- `direct_deps` already exists as a standalone function -- good
- `all_deps` uses an inner recursive function with mutable state
  (`processing`, `all_vars`, `all_var_deps`). Refactor to make per-variable
  dep computation a pure function that can be memoized.
- The circular dependency check (`processing` set) can be a separate validation
  pass.

### 4. Separate Bytecode Codegen Per-Variable

**Problem**: `Compiler::compile()` (in `codegen.rs`) walks all runlists
(initials, flows, stocks) in one pass, accumulating state
(`module_decls`, `graphical_functions`, `names`, `static_views`, etc.) into a
single `ByteCodeContext`.

**Recommendation**: Split codegen into:
- Per-variable bytecode emission (tracked per variable)
- Assembly of per-variable bytecode into a module's `ByteCodeContext` (tracked
  per module)

This is the most invasive change because the `Compiler` struct accumulates IDs
(graphical function IDs, module IDs, dimension IDs) that are position-dependent.

**Concrete steps**:
- Pre-compute the table/graphical function registry as a separate pass (it
  depends only on variable definitions, not equations)
- Pre-compute the module declaration registry similarly
- Make per-variable bytecode emission produce a `Vec<Opcode>` + metadata
  (which graphical functions / modules it references) without knowing its
  position in the final bytecode
- Final assembly resolves IDs and concatenates

**Incremental benefit**: Editing one variable's equation only re-emits that
variable's bytecode; final assembly is a cheap concatenation.

### 5. Introduce Interned Identifiers

**Problem**: `Ident<Canonical>` is used pervasively as a `String`-backed
identifier. These are cloned and hashed constantly. In salsa, interned values
get cheap copy/compare via integer IDs.

**Recommendation**: Create a `#[salsa::interned] struct VariableId` (and
`ModelId`, `DimensionId`, etc.) that wraps the canonical string. Use these
IDs throughout the pipeline.

**Incremental benefit**: Cheaper hashing/comparison, better salsa integration.
Also reduces cloning overhead in the current non-salsa code.

**Concrete steps**:
- `Ident<Canonical>` already provides a canonical, interning-like concept.
  Making it a salsa interned type is a natural evolution.
- Start by using `VariableId` in the dependency maps and runlists.
- Can be done incrementally -- replace `Ident<Canonical>` usage site by site.

### 6. Make `Project::from(datamodel::Project)` Decomposable

**Problem**: `Project::base_from` does everything in one monolithic function:
builds units context, creates all ModelStage0, creates all ModelStage1, resolves
dependencies across all models, and runs unit inference. This is the biggest
barrier to incrementalization.

**Recommendation**: Decompose into tracked queries:

```
units_context(db, project) → Context
model_stage0(db, project, model_name) → ModelStage0       // per-model
model_stage1(db, project, model_name) → ModelStage1       // per-model
module_instantiations(db, project) → HashMap<..>           // project-wide
model_with_deps(db, project, model_name) → ModelStage1    // after set_dependencies
```

**Incremental benefit**: Editing a variable in model A doesn't recompute
model B's Stage0 or Stage1.

**Concrete steps**:
- The current two-pass structure (build all Stage1, then resolve deps) works
  against per-model independence. The dependency resolution pass needs access
  to *all* models' Stage1 data.
- One approach: make `model_with_deps` depend on all model names (cheap check)
  plus only the Stage1 of models it actually references as modules.
- The topological sort of models (`model_order`) can be a separate tracked
  query.

### 7. Use Accumulators for Errors

**Problem**: Errors are stored as fields on `ModelStage1`, `Variable`, and
`Module`. This mixes error state with computed state, making it hard to know
if a result changed without comparing error lists.

**Recommendation**: Use salsa accumulators for diagnostics. Define:

```rust
#[salsa::accumulator]
pub struct CompilationDiagnostic {
    pub model: ModelId,
    pub variable: Option<VariableId>,
    pub error: Error,
}
```

Then collect diagnostics with
`compile_model::accumulated::<CompilationDiagnostic>(db, model)`.

**Incremental benefit**: Errors are a side channel -- they don't affect whether
downstream queries need recomputation. Salsa handles this naturally.

**Concrete steps**:
- Start by converting `ModelStage1::errors` and variable `errors` fields to
  accumulator usage
- The existing `EquationError` and `Error` types already carry enough context

## Suggested Implementation Order

The refactoring can be done incrementally, with each step independently
valuable:

| Phase | Change | Risk | Incremental Gain |
|-------|--------|------|------------------|
| 0 | Clone salsa into `third_party`, add to workspace | None | Foundation |
| 1 | Extract offset computation | Low | Offset stability |
| 2 | Split `set_dependencies` into composable functions | Medium | Dep caching |
| 3 | Make per-variable AST lowering standalone | Medium | Per-var caching |
| 4 | Introduce salsa db + inputs for `datamodel::Project` | Medium | Framework |
| 5 | Convert offset computation to tracked query | Low | First real query |
| 6 | Convert per-variable compilation to tracked queries | Medium | Core win |
| 7 | Convert per-variable codegen to tracked | High | Full pipeline |
| 8 | Use accumulators for errors | Medium | Cleaner errors |
| 9 | Interned identifiers | Low | Performance |

Phases 1-3 are pure refactoring with no salsa dependency -- they make the code
better regardless and can be tested and landed independently. Phases 4-9
introduce salsa incrementally.

## Architectural Considerations

### WASM Compatibility

Salsa uses `std::sync::Arc` and `std::sync::Mutex` internally, but does NOT
require threads. It works fine in single-threaded WASM. The `salsa::Storage`
type is `!Send + !Sync` by default (it becomes parallel only if you implement
`ParallelDatabase`). This matches our WASM target where `rayon` is already
disabled.

### Database Lifetime in Long-Lived Sessions

In the browser, the salsa database would live inside the WASM module's memory,
persisting across incremental edits. The typical flow:

```
User edits variable equation
  → patch applied to datamodel (existing path)
  → set_text(&mut db, var_id, new_equation)   // salsa input mutation
  → next query automatically recomputes only what changed
```

For Jupyter/pysimlin, the database lives in the `SimlinProject` Python object.

### Generic Float Type Parameter

The compiler is generic over `F: SimFloat`. Salsa tracked functions can be
generic, but each concrete instantiation is a separate query. Since we
typically only use `f64` (and occasionally `f32`), this is fine -- just
instantiate the queries for the types you need.

### Module Instantiations

Models can be instantiated as modules with different input sets, creating
`(model_name, input_set)` keyed entries. This maps well to salsa: the module
key *is* the query key. Each `(model, input_set)` combination gets its own
memoized compilation result.

## What NOT to Change

- **The VM itself**: The VM (`vm.rs`) executes compiled bytecode and doesn't
  need salsa. It's already efficient and stateless with respect to compilation.
- **The datamodel**: `datamodel::Project` is the serialization format (protobuf).
  Keep it as-is; it becomes the salsa input.
- **The interpreter** (`interpreter.rs:Simulation`): This is a reference
  implementation for correctness testing. It doesn't need incrementalization.
- **The parser** (`parser/mod.rs`): Equation parsing is already per-variable
  and fast. It can become a tracked query but is low priority.

## Estimated Impact

For a model with N variables where 1 variable is edited:

| Current | With Salsa |
|---------|-----------|
| Re-parse all N variables | Re-parse 1 variable |
| Re-lower all N ASTs | Re-lower 1 AST |
| Recompute all deps | Recompute 1 var's deps (if set unchanged, done) |
| Re-sort all runlists | Skip (memoized) |
| Re-compile all N to Expr<F> | Re-compile 1 to Expr<F> |
| Re-emit all bytecode | Re-emit 1 var's bytecode + reassemble |
| Total: O(N) | Total: O(1) amortized |

For the optimization/fitting use case (changing a constant's value), salsa
would determine at the first tracked function that the dependency set is
unchanged, and skip everything up to bytecode emission for that one constant.
Combined with the existing `set_value` override mechanism in `CompiledSimulation`,
many constant-change scenarios could skip compilation entirely.
