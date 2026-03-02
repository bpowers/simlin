# Unify PREVIOUS/INIT Dependency Extraction Design

## Summary

The simulation engine tracks how each variable depends on others across two computation phases: the initial phase (t=0) and the dt phase (each subsequent timestep). `PREVIOUS(x)` and `INIT(x)` create asymmetric dependencies -- a variable reading `PREVIOUS(x)` needs `x` only from the prior timestep, not the current one, so it should not impose an ordering constraint. Today this logic is spread across five separate AST-walking functions that share no code and independently re-implement the same traversal rules, with results stitched together by callers using up to ten separate calls per variable.

This work replaces all five functions with a single `classify_dependencies()` that walks each AST once and returns a `DepClassification` struct carrying every dependency category simultaneously. A parallel change extracts the shared predicate identifying which variables expand to stdlib modules, so that model parsing (`collect_module_idents`) and builtin routing (`builtins_visitor`) consult the same function instead of duplicating the check. The result is verified through a table-driven test matrix covering every combination of computation phase, reference form (direct, `PREVIOUS`, `INIT`, mixed), and equation shape (scalar, arrayed, subscript range, module-input branch), plus differential checks confirming that fragment and full compilation agree on phase membership.

## Definition of Done

Replace the duplicated ad-hoc AST-walk logic for PREVIOUS/INIT dependency categories with a single, unified dependency analysis pass that returns typed categories (current-step, init-only, previous-only) with per-phase granularity. Unify the module-backed classifier (`module_idents`) so model parsing and builtin routing share one authoritative source, also consumed by dependency extraction. Add table-driven invariant tests over a context matrix (phase x reference form x context) and differential checks verifying fragment compile and full compile produce equivalent dependency classifications. No changes to PREVIOUS/INIT runtime semantics or opcodes.

## Acceptance Criteria

### unify-dep-extraction.AC0: Regression Safety
- **AC0.1 Success:** All existing simulation tests (`tests/simulate.rs`) pass at each phase boundary
- **AC0.2 Success:** All existing engine unit tests (`cargo test` in `src/simlin-engine`) pass at each phase boundary
- **AC0.3 Success:** Full integration test suite passes after each phase -- no behavioral regressions introduced

### unify-dep-extraction.AC1: Single unified dependency analysis pass
- **AC1.1 Success:** `classify_dependencies()` on a scalar equation with mixed references (`PREVIOUS(a) + INIT(b) + c`) returns correct `all`, `previous_only`, `init_only`, `init_referenced`, `previous_referenced` sets in one call
- **AC1.2 Success:** `classify_dependencies()` handles `ApplyToAll` and `Arrayed` AST variants, walking all element expressions and default expressions
- **AC1.3 Success:** `IsModuleInput` branch selection works correctly when `module_inputs` is provided -- only the active branch's deps are collected
- **AC1.4 Success:** `IndexExpr2::Range` endpoints are walked and dimension-element names are filtered out
- **AC1.5 Success:** The 5 old functions (`identifier_set`, `init_referenced_idents`, `previous_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_only_referenced_idents_with_module_inputs`) are removed or reduced to thin wrappers
- **AC1.6 Edge:** Nested `PREVIOUS(PREVIOUS(x))` correctly classifies `x` as previous_only at both nesting levels

### unify-dep-extraction.AC2: Simplified db.rs consumption
- **AC2.1 Success:** `variable_direct_dependencies_impl` calls `classify_dependencies` exactly twice (dt AST + init AST) and populates `VariableDeps` from the results
- **AC2.2 Success:** `extract_implicit_var_deps` calls `classify_dependencies` exactly twice and populates `ImplicitVarDeps` from the results
- **AC2.3 Success:** Pruning logic in `model_dependency_graph_impl` produces identical dependency graphs before and after the refactoring (verified by existing integration tests passing)

### unify-dep-extraction.AC3: Authoritative module-backed classifier
- **AC3.1 Success:** `collect_module_idents()` and `builtins_visitor` PREVIOUS/INIT routing use the same predicate function for stdlib-call detection
- **AC3.2 Success:** No duplicated logic for determining whether an equation expands to a module

### unify-dep-extraction.AC4: Table-driven invariant tests
- **AC4.1 Success:** Matrix test covers all combinations: phase (dt/initial) x reference form (direct/PREVIOUS/INIT/mixed/both-lagged) x context (scalar/isModuleInput/ApplyToAll/subscript range)
- **AC4.2 Success:** Each matrix cell asserts all 5 fields of `DepClassification`
- **AC4.3 Success:** All 7 prior bug-fix edge cases have corresponding matrix entries (PREVIOUS feedback, mixed current+lagged, split by phase, INIT-only, fragment context, PREVIOUS+INIT combined, nested PREVIOUS)

### unify-dep-extraction.AC5: Differential checks
- **AC5.1 Success:** For every variable in every integration test model, the phases `compile_var_fragment` produces bytecodes for match the phases the variable appears in across dep graph runlists
- **AC5.2 Success:** Synthetic models exercising PREVIOUS feedback, INIT-only deps, nested builtins, and module-backed vars pass the differential check
- **AC5.3 Failure:** If a new variable is added that causes fragment/graph phase disagreement, the differential test catches it

## Glossary

- **PREVIOUS(x) / INIT(x)**: Builtin functions returning the value of `x` from the previous timestep or at t=0 respectively. Both break same-step ordering dependencies on `x`.
- **Initials runlist**: The ordered list of variables computed during initialization (t=0). Variables referenced by `INIT()` must appear here so their values are captured into the `initial_values` snapshot.
- **dt phase / initial phase**: The two computation phases. The initial phase runs once at t=0; the dt phase runs every subsequent timestep.
- **`Expr2`**: The AST stage after dimension resolution but before full subscript expansion. This is the stage `classify_dependencies()` operates on, with `Ast::Scalar`, `Ast::ApplyToAll`, and `Ast::Arrayed` variants.
- **`ApplyToAll`**: An `Ast` variant where a single expression applies uniformly to all subscript elements of an arrayed variable.
- **`Arrayed`**: An `Ast` variant where different subscript elements may have different expressions, with an optional default for unlisted elements (EXCEPT semantics).
- **`IsModuleInput`**: A builtin expression node inside stdlib module equations that tests whether a given input is wired. When present, dependency extraction follows only the active branch.
- **stdlib module**: A variable whose equation is a standard-library function call (SMTH1, DELAY, TREND, etc.) that expands into a synthetic sub-model with internal stocks. Module variables occupy multiple simulation slots.
- **`LoadPrev` / `LoadInitial`**: VM opcodes that read from the `prev_values` or `initial_values` snapshot buffers. Only valid for simple scalar variables, not module-backed variables.
- **`DepClassification`**: The new struct returned by `classify_dependencies()`, carrying `all`, `init_referenced`, `previous_referenced`, `previous_only`, and `init_only` sets.
- **`VariableDeps`**: Existing struct in `db.rs` aggregating all dependency categories for a variable. Populated from `DepClassification` results after this refactoring.
- **salsa**: The incremental computation framework used by the engine. `#[salsa::tracked]` functions are memoized and re-executed only when inputs change. This refactoring stays within existing salsa boundaries.
- **fragment compilation**: `compile_var_fragment()` compiles a single variable's equation independently. The differential checks compare its phase output against the full dependency graph's runlists.
- **differential check**: A test running two independent code paths over the same input and asserting equivalent results. Used in Phase 5 to verify fragment/full compile agreement.

## Architecture

Five overlapping AST-walk functions in `src/simlin-engine/src/variable.rs` (`identifier_set`, `init_referenced_idents`, `previous_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_only_referenced_idents_with_module_inputs`) are replaced by a single `classify_dependencies()` function that returns a `DepClassification` struct containing all dependency categories from one walk.

### Unified Walker

A `ClassifyVisitor` struct walks the `Expr2` AST once, maintaining two boolean flags (`in_previous`, `in_init`) and multiple accumulators:

- `all: HashSet<Ident<Canonical>>` -- every referenced identifier (replaces `identifier_set`)
- `init_referenced: BTreeSet<String>` -- direct args to `INIT()` calls (replaces `init_referenced_idents`)
- `previous_referenced: BTreeSet<String>` -- direct args to `PREVIOUS()` calls (replaces `previous_referenced_idents`)
- `non_previous: BTreeSet<String>` -- idents found outside `PREVIOUS()` context
- `non_init: BTreeSet<String>` -- idents found outside `INIT()`/`PREVIOUS()` context

After the walk, derived sets are computed via set difference:
- `previous_only = previous_referenced - non_previous` (replaces `lagged_only_previous_idents_with_module_inputs`)
- `init_only = init_referenced - non_init` (replaces `init_only_referenced_idents_with_module_inputs`)

The walker preserves all existing behaviors: dimension-name filtering from `IdentifierSetVisitor`, `IsModuleInput` branch selection via `module_inputs`, and `IndexExpr2::Range` endpoint walking.

### Result Contract

```rust
pub struct DepClassification {
    /// All referenced identifiers (current + lagged + init-only)
    pub all: HashSet<Ident<Canonical>>,
    /// Idents appearing inside any INIT() call (direct Var/Subscript args)
    pub init_referenced: BTreeSet<String>,
    /// Idents appearing inside any PREVIOUS() call (direct Var/Subscript args)
    pub previous_referenced: BTreeSet<String>,
    /// Idents referenced ONLY inside PREVIOUS() -- not outside it
    pub previous_only: BTreeSet<String>,
    /// Idents referenced ONLY inside INIT()/PREVIOUS() -- not outside either
    pub init_only: BTreeSet<String>,
}

pub fn classify_dependencies(
    ast: &Ast<Expr2>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> DepClassification
```

### Simplified Consumption in db.rs

`variable_direct_dependencies_impl()` calls `classify_dependencies` twice (dt AST, init AST) instead of 10 separate calls. `VariableDeps` fields map directly from the two `DepClassification` results. Pruning logic in `model_dependency_graph_impl()` remains in `db.rs` -- the walker classifies, the graph builder applies ordering policy.

`extract_implicit_var_deps()` in `db_implicit_deps.rs` simplifies identically -- two calls to `classify_dependencies` instead of five.

### Module-Backed Classifier Unification

`equation_is_stdlib_call()` in `model.rs` is extracted as a standalone utility shared by `collect_module_idents()` and `builtins_visitor.rs`'s routing decision. Both call the same predicate to determine "is this equation a stdlib call that expands to a module?" The `builtins_visitor`'s runtime `self.vars` extension (for modules synthesized during the current walk) remains necessary but is documented as incremental additions to the base set, using the same classification rule.

## Existing Patterns

The current codebase already uses a visitor-struct pattern for AST walking (`IdentifierSetVisitor` in `variable.rs`). The unified walker follows this same pattern, extending it with state flags rather than introducing a new abstraction.

`VariableDeps` in `db.rs` already aggregates dependency categories into a struct. The design preserves this aggregation pattern while simplifying how the struct is populated.

The table-driven test pattern exists in `variable.rs` (`test_identifier_sets`, `test_init_only_referenced_idents`) but covers only subsets of the matrix. The design extends this pattern to cover the full context matrix.

Salsa-tracked function boundaries (`variable_direct_dependencies_impl`, `model_dependency_graph_impl`, `compile_var_fragment`) are unchanged. The refactoring is internal to these functions, not across them.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Unified Walker and DepClassification

**Goal:** Replace the 5 overlapping AST-walk functions with a single `classify_dependencies()` function returning `DepClassification`.

**Components:**
- `DepClassification` struct and `classify_dependencies()` in `src/simlin-engine/src/variable.rs`
- `ClassifyVisitor` internal struct combining `IdentifierSetVisitor` state with `in_previous`/`in_init` flags
- Remove `init_referenced_idents`, `previous_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_only_referenced_idents_with_module_inputs`
- Retain `identifier_set()` as a thin wrapper over `classify_dependencies().all` if needed for callers that only want that field, or inline it

**Dependencies:** None (first phase)

**Done when:** `classify_dependencies()` returns correct results for all existing test cases from `test_identifier_sets`, `test_init_only_referenced_idents`, and `test_range_end_expressions_are_walked_in_init_previous_helpers`. All existing callers compile. Covers `unify-dep-extraction.AC1.*`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Simplify db.rs and db_implicit_deps.rs Consumption

**Goal:** Replace the multiple walker calls in `variable_direct_dependencies_impl()` and `extract_implicit_var_deps()` with calls to `classify_dependencies()`.

**Components:**
- `variable_direct_dependencies_impl()` in `src/simlin-engine/src/db.rs` -- call `classify_dependencies` twice (dt + init AST), populate `VariableDeps` from results
- `extract_implicit_var_deps()` in `src/simlin-engine/src/db_implicit_deps.rs` -- same simplification for implicit variable deps
- `VariableDeps` struct in `src/simlin-engine/src/db.rs` -- field names may be adjusted to align with `DepClassification` terminology

**Dependencies:** Phase 1

**Done when:** `variable_direct_dependencies_impl` and `extract_implicit_var_deps` each make exactly 2 calls to `classify_dependencies` (dt + init). All existing tests pass. Pruning logic in `model_dependency_graph_impl` unchanged. Covers `unify-dep-extraction.AC2.*`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Module-Backed Classifier Unification

**Goal:** Extract the shared module-classification predicate so `collect_module_idents()` and `builtins_visitor` routing use the same function.

**Components:**
- Extract `equation_is_stdlib_call()` from `src/simlin-engine/src/model.rs` into a shared location (either a standalone function in `model.rs` re-exported, or a small utility)
- `builtins_visitor.rs` routing decision in `src/simlin-engine/src/builtins_visitor.rs` -- use the shared predicate where applicable
- Document `self.vars` runtime extension in `builtins_visitor.rs` as incremental additions using the same rule

**Dependencies:** Phase 1 (shared classifier may inform walker behavior for module-backed vars)

**Done when:** `collect_module_idents()` and `builtins_visitor`'s PREVIOUS/INIT routing use the same predicate function. No duplicated classification logic. Covers `unify-dep-extraction.AC3.*`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Table-Driven Invariant Tests

**Goal:** Add comprehensive matrix tests covering phase x reference form x context for `classify_dependencies()`.

**Components:**
- `DepTestCase` struct and parameterized test in `src/simlin-engine/src/variable.rs` (or a dedicated test file)
- Matrix dimensions: phase (dt/initial), reference form (direct/PREVIOUS/INIT/mixed/both-lagged), context (scalar root/isModuleInput branch/ApplyToAll/subscript range)
- Replace existing scattered tests (`test_identifier_sets`, `test_init_only_referenced_idents`, `test_range_end_expressions_are_walked_in_init_previous_helpers`) with the unified matrix
- Each cell asserts all 5 fields of `DepClassification`

**Dependencies:** Phase 1

**Done when:** Matrix test covers all combinations from the 7 prior bug-fix commits (PREVIOUS feedback, mixed current+lagged, split by phase, INIT-only, fragment context, PREVIOUS+INIT combined, nested PREVIOUS). Covers `unify-dep-extraction.AC4.*`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Differential Checks (Fragment vs Full Compile)

**Goal:** Assert that fragment compilation and full model compilation agree on dependency classifications and phase membership for every variable.

**Components:**
- `assert_fragment_phase_agreement()` helper in `src/simlin-engine/src/db_prev_init_tests.rs` (or `db_differential_tests.rs`)
- For each variable: compare phases that `compile_var_fragment` produces bytecodes for vs phases the dep graph's runlists include the variable in
- Run over all existing integration test models in `test/`
- Run over synthetic models exercising tricky combinations (PREVIOUS feedback, INIT-only deps, nested builtins, module-backed vars)

**Dependencies:** Phases 1-2 (unified walker and simplified db consumption must be in place)

**Done when:** Differential check passes for all integration test models and synthetic edge cases. Covers `unify-dep-extraction.AC5.*`.
<!-- END_PHASE_5 -->

## Additional Considerations

**Backward compatibility:** All changes are internal to the engine. No external API, protobuf, or runtime semantics change. The `DepClassification` struct is a new internal contract; `VariableDeps` field names may shift but are only consumed within the engine.

**Salsa incrementality:** The refactoring is internal to salsa-tracked functions. Cache key boundaries (`variable_direct_dependencies_impl`, `model_dependency_graph_impl`, `compile_var_fragment`) do not change, so incremental compilation behavior is preserved.
