# Unify PREVIOUS/INIT Dependency Extraction -- Phase 2: Simplify db.rs and db_implicit_deps.rs Consumption

**Goal:** Replace the multiple walker calls in `variable_direct_dependencies_impl()` and `extract_implicit_var_deps()` with exactly 2 calls each to `classify_dependencies()` (dt AST + init AST).

**Architecture:** Each function currently makes 5-7 separate walker calls to populate `VariableDeps`/`ImplicitVarDeps`. After this phase, each function calls `classify_dependencies` twice and maps the `DepClassification` fields directly to the output struct fields. The pruning logic in `model_dependency_graph_impl()` is untouched.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 5 phases from original design (phase 2 of 5)

**Codebase verified:** 2026-03-02

---

## Acceptance Criteria Coverage

This phase implements and tests:

### unify-dep-extraction.AC2: Simplified db.rs consumption
- **unify-dep-extraction.AC2.1 Success:** `variable_direct_dependencies_impl` calls `classify_dependencies` exactly twice (dt AST + init AST) and populates `VariableDeps` from the results
- **unify-dep-extraction.AC2.2 Success:** `extract_implicit_var_deps` calls `classify_dependencies` exactly twice and populates `ImplicitVarDeps` from the results
- **unify-dep-extraction.AC2.3 Success:** Pruning logic in `model_dependency_graph_impl` produces identical dependency graphs before and after the refactoring (verified by existing integration tests passing)

### unify-dep-extraction.AC0: Regression Safety
- **unify-dep-extraction.AC0.1 Success:** All existing simulation tests (`tests/simulate.rs`) pass at each phase boundary
- **unify-dep-extraction.AC0.2 Success:** All existing engine unit tests (`cargo test` in `src/simlin-engine`) pass at each phase boundary

---

## Reference files

Read these CLAUDE.md files for project conventions before implementing:
- `/home/bpowers/src/simlin/CLAUDE.md` (project root)
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` (engine crate)

---

## Prerequisites

Phase 1 must be complete: `DepClassification` struct and `classify_dependencies()` function must exist in `src/simlin-engine/src/variable.rs`.

---

<!-- START_TASK_1 -->
### Task 1: Add `Default` impl for `DepClassification`

**Verifies:** None (infrastructure for Task 2)

**Files:**
- Modify: `src/simlin-engine/src/variable.rs` -- add `Default` derive to `DepClassification`

**Implementation:**

Add `#[derive(Default)]` to the `DepClassification` struct definition. All fields are `HashSet` or `BTreeSet` which implement `Default` (empty set). This is needed because `variable_direct_dependencies_impl` and `extract_implicit_var_deps` use `match lowered.ast() { Some(ast) => ..., None => empty }` patterns -- with `Default`, the `None` arm becomes `DepClassification::default()`.

**Verification:**

```bash
cargo test -p simlin-engine --lib
```
Expected: compiles without errors.

**Commit:** `engine: derive Default for DepClassification`

<!-- END_TASK_1 -->

<!-- START_SUBCOMPONENT_A (tasks 2-3) -->
<!-- START_TASK_2 -->
### Task 2: Simplify `variable_direct_dependencies_impl` in db.rs

**Verifies:** unify-dep-extraction.AC2.1, unify-dep-extraction.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/db.rs:850-925` -- replace the non-Module arm of `variable_direct_dependencies_impl`

**Implementation:**

Replace the 7 walker calls (lines 871-912) in the `_ =>` arm with exactly 2 calls to `classify_dependencies`. The Module arm (lines 833-848) is unchanged.

The current code (lines 871-912) calls:
1. `identifier_set(dt_ast, dims, module_inputs)` -> `dt_deps`
2. `identifier_set(init_ast, dims, module_inputs)` -> `initial_deps`
3. `extract_implicit_var_deps(...)` -> `implicit_vars`
4. `init_referenced_idents(dt_ast)` -> `init_referenced_vars`
5. `init_only_referenced_idents_with_module_inputs(dt_ast, module_inputs)` -> `dt_init_only_referenced_vars`
6. `lagged_only_previous_idents_with_module_inputs(dt_ast, module_inputs)` -> `dt_previous_referenced_vars`
7. `lagged_only_previous_idents_with_module_inputs(init_ast, module_inputs)` -> `initial_previous_referenced_vars`

Replace with:

```rust
// Two calls to classify_dependencies replace 7 separate walker calls.
let dt_classification = match lowered.ast() {
    Some(ast) => {
        crate::variable::classify_dependencies(ast, &converted_dims, module_inputs)
    }
    None => crate::variable::DepClassification::default(),
};
let init_classification = match lowered.init_ast() {
    Some(ast) => {
        crate::variable::classify_dependencies(ast, &converted_dims, module_inputs)
    }
    None => crate::variable::DepClassification::default(),
};

let implicit_vars =
    extract_implicit_var_deps(parsed, &dims, &dim_context, module_inputs);

VariableDeps {
    dt_deps: dt_classification
        .all
        .into_iter()
        .map(|id| id.to_string())
        .collect(),
    initial_deps: init_classification
        .all
        .into_iter()
        .map(|id| id.to_string())
        .collect(),
    implicit_vars,
    init_referenced_vars: dt_classification.init_referenced,
    dt_init_only_referenced_vars: dt_classification.init_only,
    dt_previous_referenced_vars: dt_classification.previous_only,
    initial_previous_referenced_vars: init_classification.previous_only,
}
```

**Field mapping from DepClassification to VariableDeps:**

| VariableDeps field | Source | DepClassification field | Conversion |
|---|---|---|---|
| `dt_deps` | dt | `.all` | `HashSet<Ident>` -> `BTreeSet<String>` via `.into_iter().map(\|id\| id.to_string()).collect()` |
| `initial_deps` | init | `.all` | same conversion |
| `implicit_vars` | `extract_implicit_var_deps()` | N/A | unchanged |
| `init_referenced_vars` | dt | `.init_referenced` | direct (already `BTreeSet<String>`) |
| `dt_init_only_referenced_vars` | dt | `.init_only` | direct |
| `dt_previous_referenced_vars` | dt | `.previous_only` | direct |
| `initial_previous_referenced_vars` | init | `.previous_only` | direct |

Remove the `crate::variable::identifier_set`, `crate::variable::init_referenced_idents`, `crate::variable::init_only_referenced_idents_with_module_inputs`, and `crate::variable::lagged_only_previous_idents_with_module_inputs` calls from this function. Do NOT remove the imports at the top of db.rs yet -- other code may still use them.

**Note on wrapper function retention:** The old standalone functions (`identifier_set`, `init_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`, `init_only_referenced_idents_with_module_inputs`) are converted to thin wrappers in Phase 1 Task 2. They remain available because callers in `model.rs`, `ltm.rs`, `ltm_augment.rs`, and `db_ltm.rs` still use them. Only the calls within `variable_direct_dependencies_impl` are replaced here. Removing the wrapper functions entirely is out of scope for this refactoring.

**Testing:**

Existing tests verify that `VariableDeps` is populated correctly through the salsa pipeline:
- unify-dep-extraction.AC2.1: `variable_direct_dependencies_impl` now makes exactly 2 `classify_dependencies` calls
- unify-dep-extraction.AC2.3: `model_dependency_graph_impl` (lines 1111+) consumes `VariableDeps` fields for pruning -- unchanged code, so identical dep graphs

**Verification:**

```bash
cargo test -p simlin-engine
```
Expected: all unit tests pass.

```bash
cargo test -p simlin-engine --features file_io
```
Expected: all integration tests pass (confirms dependency graphs are identical).

**Commit:** `engine: simplify variable_direct_dependencies_impl with classify_dependencies`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Simplify `extract_implicit_var_deps` in db_implicit_deps.rs

**Verifies:** unify-dep-extraction.AC2.2

**Files:**
- Modify: `src/simlin-engine/src/db_implicit_deps.rs:86-120` -- replace the 5 walker calls per implicit var with 2 `classify_dependencies` calls

**Implementation:**

Inside the `.map(|implicit_var| { ... })` closure, after `let lowered = crate::model::lower_variable(...)` (line 84), replace the 5 walker calls (lines 86-120) with:

```rust
let dt_classification = match lowered.ast() {
    Some(ast) => {
        crate::variable::classify_dependencies(ast, &converted_dims, module_inputs)
    }
    None => crate::variable::DepClassification::default(),
};
let init_classification = match lowered.init_ast() {
    Some(ast) => {
        crate::variable::classify_dependencies(ast, &converted_dims, module_inputs)
    }
    None => crate::variable::DepClassification::default(),
};

ImplicitVarDeps {
    name: implicit_name,
    is_stock: parsed_implicit.is_stock(),
    is_module,
    model_name,
    dt_deps: dt_classification
        .all
        .into_iter()
        .map(|id| id.to_string())
        .collect(),
    initial_deps: init_classification
        .all
        .into_iter()
        .map(|id| id.to_string())
        .collect(),
    dt_init_only_referenced_vars: dt_classification.init_only,
    dt_previous_referenced_vars: dt_classification.previous_only,
    initial_previous_referenced_vars: init_classification.previous_only,
}
```

**Field mapping from DepClassification to ImplicitVarDeps:**

| ImplicitVarDeps field | Source | DepClassification field | Conversion |
|---|---|---|---|
| `dt_deps` | dt | `.all` | `HashSet<Ident>` -> `BTreeSet<String>` |
| `initial_deps` | init | `.all` | same conversion |
| `dt_init_only_referenced_vars` | dt | `.init_only` | direct |
| `dt_previous_referenced_vars` | dt | `.previous_only` | direct |
| `initial_previous_referenced_vars` | init | `.previous_only` | direct |

Note: `ImplicitVarDeps` has no `init_referenced_vars` field (unlike `VariableDeps`), so that DepClassification field is unused here.

The Module early-return path (lines 50-67) remains unchanged -- modules have no AST and derive deps from `m.references`.

Remove the `crate::variable::identifier_set`, `crate::variable::init_only_referenced_idents_with_module_inputs`, and `crate::variable::lagged_only_previous_idents_with_module_inputs` calls from this function.

**Testing:**

Existing integration tests exercise implicit variable deps through stdlib function expansion (SMOOTH, DELAY, etc.) in test models. The pruning logic in `model_dependency_graph_impl` iterates `deps.implicit_vars` (lines 1211-1226 of db.rs) and applies the same field-based pruning.

**Verification:**

```bash
cargo test -p simlin-engine
```

```bash
cargo test -p simlin-engine --features file_io
```
Expected: all tests pass. Integration tests with SMOOTH/DELAY models confirm implicit var deps are correct.

**Commit:** `engine: simplify extract_implicit_var_deps with classify_dependencies`

<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->
