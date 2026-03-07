# Finish Salsa Migration Implementation Plan

**Goal:** Make the incremental salsa compilation pipeline the sole compilation path, then delete the monolithic code.

**Architecture:** The salsa-based incremental pipeline (`compile_project_incremental` / `assemble_simulation`) is already the production default. This plan migrates remaining callers, tests, and infrastructure, then deletes the monolithic path (`Project::from`, `Simulation::compile`, `compile_project`).

**Tech Stack:** Rust, salsa (incremental computation framework)

**Scope:** 7 phases from original design (phases 1-7)

**Codebase verified:** 2026-03-06

---

## Acceptance Criteria Coverage

This phase implements and tests:

### finish-salsa-migration.AC8: Dimension-granularity invalidation (TD18)
- **finish-salsa-migration.AC8.1 Success:** Changing dimension A does not trigger re-parse of a scalar variable.
- **finish-salsa-migration.AC8.2 Success:** Changing dimension A does not trigger re-parse of a variable that only references dimension B.
- **finish-salsa-migration.AC8.3 Success:** Changing dimension A does trigger re-parse of a variable that references dimension A (including via `maps_to` chains).

---

## Phase 7: Dimension-Granularity Invalidation (TD18)

**Phase type:** Functionality -- adding a new tracked function and modifying the parse pipeline.

**Note on line numbers:** Line numbers referenced below are approximate and may shift as earlier phases modify files. Use function name search rather than relying on exact line numbers.

**Prerequisites:** Phase 3 (parse context unification -- only `parse_source_variable_with_module_context` exists).

**Background:**

Currently, when any dimension in the project changes, ALL variables are re-parsed because `parse_source_variable_impl` (db.rs:717) reads the full `project_datamodel_dims(db, project)` which is invalidated when any dimension changes. The goal is to filter dimensions per-variable so only variables that actually reference the changed dimension are re-parsed.

**Architecture of the change:**

```
Before (coarse invalidation):
  parse_source_variable_impl
    -> project_datamodel_dims(db, project)  // reads ALL dimensions
    -> parse_var_with_module_context(all_dims, ...)

After (fine-grained invalidation):
  parse_source_variable_impl
    -> variable_relevant_dimensions(db, var)     // NEW: extracts dim names from SourceEquation
    -> project_relevant_dims(db, project, var)   // NEW: filters to only relevant dims
    -> parse_var_with_module_context(filtered_dims, ...)
```

**Key types:**
- `SourceEquation::Scalar(String)` -- no dimensions (empty set)
- `SourceEquation::ApplyToAll(Vec<String>, String)` -- dim_names in first field
- `SourceEquation::Arrayed(Vec<String>, Vec<...>, Option<String>)` -- dim_names in first field
- `SourceDimension` has `maps_to: Option<String>` and `mappings: Vec<SourceDimensionMapping>`
- `DimensionsContext::from(&[datamodel::Dimension])` builds lookup from slice

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Add variable_relevant_dimensions tracked function

**Verifies:** finish-salsa-migration.AC8.1, finish-salsa-migration.AC8.2

**Files:**
- Modify: `src/simlin-engine/src/db.rs`

**Implementation:**

Add a new `#[salsa::tracked]` function that extracts the dimension names a variable references from its `SourceEquation`:

```rust
#[salsa::tracked(returns(ref))]
pub fn variable_relevant_dimensions(
    db: &dyn Db,
    var: SourceVariable,
) -> BTreeSet<String> {
    match var.equation(db) {
        SourceEquation::Scalar(_) => BTreeSet::new(),
        SourceEquation::ApplyToAll(dim_names, _) => dim_names.iter().cloned().collect(),
        SourceEquation::Arrayed(dim_names, _, _) => dim_names.iter().cloned().collect(),
    }
}
```

This function reads only `var.equation(db)`, NOT `project.dimensions(db)`. When a dimension changes but the variable's equation text doesn't, salsa won't re-execute this function (the input hasn't changed). When the returned set changes (e.g., variable goes from scalar to arrayed), downstream functions that depend on it will re-execute.

Place this function near the existing `variable_dimensions` function (db.rs:2888) for grouping.

**Verification:**
```bash
cargo check -p simlin-engine
```
Expected: Compiles.

**Commit:** `engine: add variable_relevant_dimensions tracked function`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add transitive maps_to expansion

**Verifies:** finish-salsa-migration.AC8.3

**Files:**
- Modify: `src/simlin-engine/src/db.rs`

**Implementation:**

Add a helper function that expands a set of dimension names to include transitive `maps_to` targets:

```rust
fn expand_maps_to_chains(
    dim_names: &BTreeSet<String>,
    all_dims: &[SourceDimension],
) -> BTreeSet<String> {
    let mut expanded = dim_names.clone();
    let dim_map: HashMap<&str, &SourceDimension> = all_dims
        .iter()
        .map(|d| (d.name(db).as_str(), d))
        .collect();

    let mut to_visit: Vec<&str> = dim_names.iter().map(|s| s.as_str()).collect();
    while let Some(name) = to_visit.pop() {
        if let Some(dim) = dim_map.get(name) {
            // Check maps_to field
            if let Some(ref target) = dim.maps_to {
                if expanded.insert(target.clone()) {
                    to_visit.push(target.as_str());
                }
            }
            // Check mappings field
            for mapping in &dim.mappings {
                if expanded.insert(mapping.target.clone()) {
                    to_visit.push(mapping.target.as_str());
                }
            }
        }
    }
    expanded
}
```

This follows `maps_to` chains transitively (A -> B -> C includes all three in the set). The `BTreeSet::insert` returning `false` on duplicates prevents infinite loops in circular mappings.

**Design decision:** This function is a pure helper that accepts pre-fetched `&[SourceDimension]` data, NOT a salsa-tracked function. The caller (`parse_source_variable_impl`) reads `project.dimensions(db)` to get the `SourceDimension` list and passes it to this helper. Salsa tracks the read of `project.dimensions(db)` at the caller level, which is sufficient for invalidation -- if any dimension's `maps_to` changes, `project.dimensions(db)` is invalidated, which re-triggers the caller.

**Verification:**
```bash
cargo check -p simlin-engine
```
Expected: Compiles.

**Commit:** `engine: add transitive maps_to chain expansion for dimension filtering`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Filter dimensions in parse_source_variable_impl

**Verifies:** finish-salsa-migration.AC8.1, finish-salsa-migration.AC8.2, finish-salsa-migration.AC8.3

**Files:**
- Modify: `src/simlin-engine/src/db.rs` (`parse_source_variable_impl` at line ~717)

**Implementation:**

Modify `parse_source_variable_impl` to filter the project dimensions through the variable's relevant dimensions before passing them to `parse_var_with_module_context`.

Current flow (db.rs:723):
```rust
let dims = project_datamodel_dims(db, project);
// ... then dims is passed to parse_var_with_module_context
```

New flow:
```rust
let relevant_dim_names = variable_relevant_dimensions(db, var);
if relevant_dim_names.is_empty() {
    // Scalar variable -- pass empty dimensions, immune to all dimension changes
    let dims: Vec<datamodel::Dimension> = vec![];
    // ... pass empty dims to parse_var_with_module_context
} else {
    // Expand relevant dims with maps_to chains
    let all_source_dims = project.dimensions(db);
    let expanded = expand_maps_to_chains(&relevant_dim_names, all_source_dims);
    // Filter project dimensions to only relevant ones
    let dims: Vec<datamodel::Dimension> = project_datamodel_dims(db, project)
        .iter()
        .filter(|d| expanded.contains(&d.name))
        .cloned()
        .collect();
    // ... pass filtered dims to parse_var_with_module_context
}
```

**Important salsa invalidation semantics:** The key is that `variable_relevant_dimensions` reads only `var.equation(db)`, NOT `project.dimensions(db)`. So for scalar variables, the dimension read is completely avoided -- changing ANY dimension won't invalidate the scalar variable's parse result.

For arrayed variables, we still read `project.dimensions(db)` (to expand maps_to), but salsa tracks the actual values returned. If dimension A changes but the variable only references dimension B (which didn't change), salsa's memoization check on the filtered result will see the same output and won't propagate invalidation.

**Alternative approach:** Instead of filtering after `project_datamodel_dims`, create a new tracked function `project_relevant_dims(db, project, var)` that combines the relevance check and dimension fetching. This gives salsa finer-grained memoization boundaries. Evaluate which approach gives better cache behavior.

**Verification:**
```bash
cargo check -p simlin-engine
cargo test -p simlin-engine
```
Expected: Compiles and all existing tests pass (behavior is unchanged -- only invalidation granularity changes).

**Commit:** `engine: filter dimensions per-variable in parse pipeline for granular invalidation`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-5) -->

<!-- START_TASK_4 -->
### Task 4: Add tests for dimension-granularity invalidation

**Verifies:** finish-salsa-migration.AC8.1, finish-salsa-migration.AC8.2, finish-salsa-migration.AC8.3

**Files:**
- Modify: `src/simlin-engine/src/db_tests.rs` (or create `src/simlin-engine/src/db_dimension_invalidation_tests.rs`)

**Implementation:**

Add tests that verify the invalidation granularity using salsa's event tracking or by observing cache hits.

**Test 1: Scalar variable immune to dimension changes (AC8.1)**

1. Create a model with dimensions A and B, a scalar variable `x = 10`, and an arrayed variable `y[A] = x + 1`
2. Sync to SimlinDb, parse `x` (triggers `parse_source_variable_with_module_context`)
3. Add/modify dimension A (e.g., add a new element)
4. Re-sync incrementally
5. Verify that `parse_source_variable_with_module_context` for `x` returns the cached result (salsa did NOT re-execute it)

**Test 2: Arrayed variable with different dimension immune (AC8.2)**

1. Create a model with dimensions A and B, variable `y[B] = 5`
2. Sync, parse `y`
3. Modify dimension A (add element)
4. Re-sync incrementally
5. Verify `y`'s parse result is cached (dimension A change didn't affect B-only variable)

**Test 3: Arrayed variable with same dimension re-parsed (AC8.3)**

1. Create a model with dimensions A and B, variable `y[A] = 5`
2. Sync, parse `y`
3. Modify dimension A (add element)
4. Re-sync incrementally
5. Verify `y`'s parse result was re-computed (dimension A changed and y references A)

**Test 4: maps_to chain triggers re-parse (AC8.3)**

1. Create a model with dimension A that maps_to dimension B, variable `y[A] = 5`
2. Sync, parse `y`
3. Modify dimension B (which A maps to)
4. Re-sync incrementally
5. Verify `y`'s parse result was re-computed (B changed, and y references A which maps_to B)

**Verifying salsa cache behavior:** Use salsa event logging if available, or compare the `salsa::Id` of the returned `ParsedVariableResult` before and after the dimension change. If the IDs match, the result was cached (no re-execution).

**Testing:**
- finish-salsa-migration.AC8.1: Test 1 verifies scalar immunity
- finish-salsa-migration.AC8.2: Test 2 verifies cross-dimension immunity
- finish-salsa-migration.AC8.3: Tests 3 and 4 verify same-dimension and maps_to re-parse

**Verification:**
```bash
cargo test -p simlin-engine test_dimension_invalidation
```
Expected: All 4 tests pass.

**Commit:** `engine: add tests for dimension-granularity salsa invalidation`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Full test suite and final verification

**Verifies:** finish-salsa-migration.AC8, finish-salsa-migration.AC7.4

**Step 1: Run all engine tests**

```bash
cargo test -p simlin-engine
```

Expected: All tests pass.

**Step 2: Run all libsimlin tests**

```bash
cargo test -p libsimlin
```

Expected: All tests pass.

**Step 3: Run full test suite**

```bash
cargo test
```

Expected: All tests pass across the entire workspace.

**Commit:** (no commit -- verification only)
<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_B -->
