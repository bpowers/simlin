# Unify PREVIOUS/INIT Dependency Extraction -- Phase 4: Table-Driven Invariant Tests

**Goal:** Add comprehensive matrix tests covering reference form x context for `classify_dependencies()`, replacing scattered existing tests and ensuring all 7 prior bug-fix edge cases have coverage.

**Architecture:** A table-driven test with a `DepTestCase` struct. Each case specifies an AST (constructed either via `parse_equation`+`lower_ast` or directly), dimensions, module_inputs, and expected values for all 5 fields of `DepClassification`. The test matrix covers all combinations of reference form (direct, PREVIOUS, INIT, mixed, both-lagged) and context (scalar, isModuleInput, ApplyToAll, subscript range), plus edge cases from 7 prior bug-fix commits.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 5 phases from original design (phase 4 of 5)

**Codebase verified:** 2026-03-02

---

## Acceptance Criteria Coverage

This phase implements and tests:

### unify-dep-extraction.AC4: Table-driven invariant tests
- **unify-dep-extraction.AC4.1 Success:** Matrix test covers all combinations: phase (dt/initial) x reference form (direct/PREVIOUS/INIT/mixed/both-lagged) x context (scalar/isModuleInput/ApplyToAll/subscript range)
- **unify-dep-extraction.AC4.2 Success:** Each matrix cell asserts all 5 fields of `DepClassification`
- **unify-dep-extraction.AC4.3 Success:** All 7 prior bug-fix edge cases have corresponding matrix entries (PREVIOUS feedback, mixed current+lagged, split by phase, INIT-only, fragment context, PREVIOUS+INIT combined, nested PREVIOUS)

### unify-dep-extraction.AC0: Regression Safety
- **unify-dep-extraction.AC0.2 Success:** All existing engine unit tests (`cargo test` in `src/simlin-engine`) pass at each phase boundary

---

## Reference files

Read these CLAUDE.md files for project conventions before implementing:
- `/home/bpowers/src/simlin/CLAUDE.md` (project root)
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` (engine crate)

---

## Prerequisites

Phase 1 must be complete: `DepClassification` and `classify_dependencies()` exist in `variable.rs`.

---

## Prior bug-fix commits (for edge case test design)

These 7 commits introduced the dependency extraction logic being unified. Each needs a corresponding test case:

| # | Edge case | Commit | Key behavior |
|---|-----------|--------|-------------|
| 1 | PREVIOUS feedback | `7a9db2a5` | `PREVIOUS(b)` -> `b` is previous_only, NOT in non-previous deps |
| 2 | Mixed current+lagged | `ae9f4ed9` | `PREVIOUS(b) + b` -> `b` in previous_referenced AND non_previous, so NOT previous_only |
| 3 | Split by phase | `09ae1b33` | Same equation classified differently when used as dt AST vs init AST (tested by calling `classify_dependencies` on each) |
| 4 | INIT-only | `b0580011` | `INIT(b)` -> `b` is init_only, pruned from dt ordering |
| 5 | Fragment context | `55ebef55` | `INIT(b)` -> `b` in `all` set (fragment needs it even though dep graph prunes ordering) |
| 6 | PREVIOUS+INIT combined | `c537bb2d` | `PREVIOUS(b) + INIT(b)` -> `b` is init_only (PREVIOUS context also counts as init-excluded) |
| 7 | Nested PREVIOUS | `0aecdfbb` | `PREVIOUS(PREVIOUS(x))` -> `x` is previous_only at both nesting levels |

---

<!-- START_TASK_1 -->
### Task 1: Replace scattered tests with unified matrix test for `classify_dependencies`

**Verifies:** unify-dep-extraction.AC4.1, unify-dep-extraction.AC4.2, unify-dep-extraction.AC4.3

**Files:**
- Modify: `src/simlin-engine/src/variable.rs` -- replace `test_identifier_sets` (line 1137), `test_init_only_referenced_idents` (line 1202), and `test_range_end_expressions_are_walked_in_init_previous_helpers` (line 1233) with a single `test_classify_dependencies_matrix` test

**Implementation:**

Remove the three old tests and add a single comprehensive matrix test. The test uses a `DepTestCase` struct and iterates over a `cases` array.

**Test case struct:**

```rust
struct DepTestCase {
    /// Human-readable label for assertion messages
    label: &'static str,
    /// The AST to classify
    ast: Ast<Expr2>,
    /// Dimensions for filtering (empty for most cases)
    dimensions: Vec<Dimension>,
    /// Module inputs for IsModuleInput branch selection (None for most cases)
    module_inputs: Option<BTreeSet<Ident<Canonical>>>,
    /// Expected: all referenced identifiers (as strings, for easy comparison)
    expected_all: HashSet<&'static str>,
    /// Expected: direct INIT() argument names
    expected_init_referenced: BTreeSet<&'static str>,
    /// Expected: direct PREVIOUS() argument names
    expected_previous_referenced: BTreeSet<&'static str>,
    /// Expected: idents ONLY inside PREVIOUS (not outside)
    expected_previous_only: BTreeSet<&'static str>,
    /// Expected: idents ONLY inside INIT/PREVIOUS (not outside either)
    expected_init_only: BTreeSet<&'static str>,
}
```

**Test runner:**

For each case, call `classify_dependencies(&case.ast, &case.dimensions, case.module_inputs.as_ref())` and assert all 5 fields match expected values. Convert `DepClassification.all` from `HashSet<Ident<Canonical>>` to `HashSet<&str>` for comparison (via `.iter().map(|id| id.as_str()).collect()`). For BTreeSet fields, compare strings directly.

Include the case label in assertion messages for diagnosability:
```rust
assert_eq!(expected_all, got_all, "case '{}': all", case.label);
```

**AST construction approach:**

Use two patterns depending on the case:

1. **Text-based** (for scalar equations): Parse via `parse_equation(&datamodel::Equation::Scalar(eqn.to_owned()), &[], false, None)` then `lower_ast(&scope, ast)`. Reuse the `ScopeStage0` setup from the existing `test_identifier_sets` test.

2. **Direct construction** (for subscript ranges, arrayed, and cases needing precise AST structure): Build `Expr2` nodes directly using `Loc::new(0, 1)`, `Ident::new(...)`, `BuiltinFn::Previous(Box::new(...))`, etc. This is the pattern from `test_range_end_expressions_are_walked_in_init_previous_helpers`.

**Helper to build a text-based scalar AST:**

```rust
fn scalar_ast(eqn: &str) -> Ast<Expr2> {
    let (ast, err) = parse_equation(
        &datamodel::Equation::Scalar(eqn.to_owned()),
        &[],
        false,
        None,
    );
    assert!(err.is_empty(), "parse error in test equation: {eqn}");
    let scope = ScopeStage0 {
        models: &Default::default(),
        dimensions: &Default::default(),
        model_name: "test",
    };
    lower_ast(&scope, ast.unwrap()).unwrap()
}
```

**Test case matrix:**

The cases below cover the full matrix of **reference form x context**. The phase dimension (dt vs initial) is intentionally collapsed: `classify_dependencies` is phase-agnostic -- it classifies a single AST regardless of whether the caller passes a dt AST or init AST. The "split by phase" behavior (edge case 3) is in how `db.rs` assigns results from two separate `classify_dependencies` calls to different `VariableDeps` fields. This is tested by the `split_phase_dt`/`split_phase_init` case pair below and by Phase 5's differential checks.

Group labels indicate the matrix dimension being tested.

**Reference form: direct (no PREVIOUS/INIT)**

| Label | Equation/AST | Context | Expected all | init_ref | prev_ref | prev_only | init_only |
|---|---|---|---|---|---|---|---|
| `direct_scalar` | `a + b` | scalar | {a, b} | {} | {} | {} | {} |
| `direct_a2a` | `a + b` wrapped in `ApplyToAll(dim, ...)` | ApplyToAll | {a, b} | {} | {} | {} | {} |
| `direct_arrayed` | Arrayed with element "e1"=`a`, default=`b` | Arrayed | {a, b} | {} | {} | {} | {} |
| `direct_ismoduleinput` | `if isModuleInput(input) then a else b` with module_inputs={input} | isModuleInput | {a} | {} | {} | {} | {} |
| `direct_range` | `arr[1:CONST]` (subscript range with both endpoints walked) | subscript range | {arr, const} | {} | {} | {} | {} |

**Reference form: PREVIOUS only**

| Label | Equation/AST | Context | Expected all | init_ref | prev_ref | prev_only | init_only |
|---|---|---|---|---|---|---|---|
| `previous_scalar` (edge case 1) | `PREVIOUS(b)` | scalar | {b} | {} | {b} | {b} | {} |
| `previous_a2a` | `PREVIOUS(b)` in ApplyToAll | ApplyToAll | {b} | {} | {b} | {b} | {} |
| `previous_ismoduleinput` | `if isModuleInput(input) then PREVIOUS(a) else b` with module_inputs={input} | isModuleInput | {a} | {} | {a} | {a} | {} |
| `previous_range` | `arr[1:PREVIOUS(lagged)]` (range endpoint) | subscript range | {arr, lagged} | {} | {lagged} | {lagged} | {} |

**Reference form: INIT only**

| Label | Equation/AST | Context | Expected all | init_ref | prev_ref | prev_only | init_only |
|---|---|---|---|---|---|---|---|
| `init_scalar` (edge case 4, 5) | `INIT(b)` | scalar | {b} | {b} | {} | {} | {b} |
| `init_a2a` | `INIT(b)` in ApplyToAll | ApplyToAll | {b} | {b} | {} | {} | {b} |
| `init_ismoduleinput` | `if isModuleInput(input) then INIT(a) else b` with module_inputs={input} | isModuleInput | {a} | {a} | {} | {} | {a} |
| `init_range` | `arr[1:INIT(seed)]` (range endpoint) | subscript range | {arr, seed} | {seed} | {} | {} | {seed} |

**Reference form: mixed (current + lagged)**

| Label | Equation/AST | Context | Expected all | init_ref | prev_ref | prev_only | init_only |
|---|---|---|---|---|---|---|---|
| `mixed_prev_current` (edge case 2) | `PREVIOUS(b) + b` | scalar | {b} | {} | {b} | {} | {} |
| `mixed_init_current` | `INIT(b) + b` | scalar | {b} | {b} | {} | {} | {} |
| `mixed_prev_current_a2a` | `PREVIOUS(b) + b` in ApplyToAll | ApplyToAll | {b} | {} | {b} | {} | {} |
| `mixed_prev_current_ismoduleinput` | `if isModuleInput(input) then PREVIOUS(a) + a else b` with module_inputs={input} | isModuleInput | {a} | {} | {a} | {} | {} |

**Reference form: both-lagged (PREVIOUS + INIT)**

| Label | Equation/AST | Context | Expected all | init_ref | prev_ref | prev_only | init_only |
|---|---|---|---|---|---|---|---|
| `both_lagged_scalar` (edge case 6) | `PREVIOUS(b) + INIT(b)` | scalar | {b} | {b} | {b} | {b} | {b} |
| `both_lagged_different` | `PREVIOUS(a) + INIT(b)` | scalar | {a, b} | {b} | {a} | {a} | {b} |
| `both_lagged_a2a` | `PREVIOUS(b) + INIT(b)` in ApplyToAll | ApplyToAll | {b} | {b} | {b} | {b} | {b} |

**Additional edge cases**

| Label | Equation/AST | Context | Expected all | init_ref | prev_ref | prev_only | init_only |
|---|---|---|---|---|---|---|---|
| `nested_previous` (edge case 7) | `PREVIOUS(PREVIOUS(x))` | scalar | {x} | {} | {x} | {x} | {} |
| `init_with_dotted_ref` | `INIT(m.out1) + m.out2` | scalar | {m·out1, m·out2} | {m·out1} | {} | {} | {} |
| `previous_plus_init_same_var` (edge case 6 variant) | `PREVIOUS(b) + INIT(b)` | scalar | {b} | {b} | {b} | {b} | {b} |
| `dim_filtering` | `a + foo` with dim1={foo} | scalar w/ dims | {a} | {} | {} | {} | {} |
| `ismoduleinput_else_branch` | `if isModuleInput(input) then a else b` with module_inputs={input} | isModuleInput | {a} | {} | {} | {} | {} |
| `ismoduleinput_no_pruning` | `if isModuleInput(input) then a else b` with module_inputs=None | no pruning | {input, a, b} | {} | {} | {} | {} |

**Notes on edge case 3 (split by phase):** `classify_dependencies` is phase-agnostic -- the caller determines which AST to pass (dt or init). This edge case is covered by Phase 5's differential checks. Include a test case pair here that demonstrates the same equation produces correct classifications regardless of context, named `split_phase_dt` and `split_phase_init` -- both use the same `PREVIOUS(b) + c` equation, confirming the result is identical. The "split" behavior is in how `db.rs` assigns the results to different `VariableDeps` fields.

**Notes on edge case 5 (fragment context):** The `all` field contains ALL referenced identifiers including those inside INIT/PREVIOUS. This is what `compile_var_fragment` uses for its `dt_deps`. The test cases already verify that `all` includes INIT/PREVIOUS args (e.g., `init_scalar` has `all={b}`). The fragment context invariant is: `all` is a superset of `init_referenced ∪ previous_referenced`. Include an explicit assertion of this invariant in the test runner.

**Testing:**

All matrix cells assert all 5 fields of `DepClassification`. The test runner also asserts the structural invariant: `all` (as strings) is a superset of `init_referenced ∪ previous_referenced`.

**Verification:**

```bash
cargo test -p simlin-engine test_classify_dependencies_matrix
```
Expected: all cases pass.

```bash
cargo test -p simlin-engine
```
Expected: all tests pass (including removal of 3 old tests -- their coverage is subsumed by the matrix).

**Commit:** `engine: add table-driven classify_dependencies matrix test`

<!-- END_TASK_1 -->
