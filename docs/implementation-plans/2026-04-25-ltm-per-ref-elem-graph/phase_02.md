# Phase 2 — AST-Walking Element Graph Builder

**Goal:** Replace `classify_element_dependency` + `expand_edge_to_elements` in `model_element_causal_edges` with a single per-target AST walker that emits element edges per AST occurrence. Phase 1's red tests for AC1.1 / AC1.3 turn green.

**Architecture:** The new walker recursively descends a target variable's `Expr2` AST. For each `Var`/`Subscript` reference whose ident matches a source-edge, it classifies the access shape (Bare / Wildcard / FixedIndex / DynamicIndex) and emits the element edges that shape implies. Edge emission is keyed by `(source, target, RefShape)` and unioned into the result's edge map.

**Tech Stack:** Rust, salsa-tracked `model_element_causal_edges`, `Expr2` AST, `walk_builtin_expr` with `BuiltinContents`.

**Codebase verified:** 2026-04-25 (Phase 2 codebase-investigator confirmed: `ElementDependencyKind` is private to `db_analysis.rs`; only `model_element_causal_edges` calls it; helpers `cartesian_element_names`, `format_element_name`, `format_multi_element_name`, `dimension_element_names` stay; polarity analysis does not depend on `ElementDependencyKind`; existing `db_element_graph_tests.rs` uses Wildcard patterns and is correct under both old and new behavior).

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### ltm-per-ref-elem-graph.AC1: Element-edge structure is per-AST-reference
- **ltm-per-ref-elem-graph.AC1.1 Fixed-index broadcast not over-expanded:** For `relative_pop[R] = population / population[NYC]` over a dimension R of size N, the element-graph must contain exactly the diagonal same-element edges plus the broadcast-from-NYC edges. Total unique edges: 2N − 1, not N².
- **ltm-per-ref-elem-graph.AC1.2 Wildcard reducer remains all-pairs:** For `share[R] = population / SUM(population[*])`, the wildcard reducer continues to emit all-pairs edges *in addition to* the diagonal SameElement edges.
- **ltm-per-ref-elem-graph.AC1.3 Cross-element fixture edge set:** For `test/cross_element_ltm/cross_element.stmx`, the element graph must contain the truthful broadcast edges and not the spurious all-pairs edges.
- **ltm-per-ref-elem-graph.AC1.4 Variable-level projection invariant:** For every project, the variable-level projection of element edges equals `model_causal_edges`.
- **ltm-per-ref-elem-graph.AC1.5 Multidim partial-fixed conservative:** Multi-dimensional sources with mixed literal+wildcard indices treat as Wildcard.

---

## Implementation Tasks

The phase begins with the new walker living alongside `classify_element_dependency` (parallel implementation), pivots `model_element_causal_edges` to call it, then deletes the old code. This three-step approach keeps each commit independently buildable and testable.

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Promote `RefShape` to a real enum and add `ReferenceSite` data type

**Verifies:** none directly (sets up Tasks 2–6)

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` (or `src/simlin-engine/src/db_analysis.rs`, depending on Phase 1's stub location — pick the same file Phase 1 used)

**Implementation:**
Phase 1 introduced a `RefShape` stub. Promote it to a final enum with proper documentation and `Debug`/`Clone`/`PartialEq`/`Eq`/`Hash` derives. Place it in `db_analysis.rs` near the top of the file (before `ElementDependencyKind`, which is removed at the end of Phase 2). Add a sibling `ReferenceSite` struct describing one occurrence:

```rust
/// How a source variable is accessed at a single AST reference site.
///
/// Distinguishes bare references (in scalar or A2A context), wildcard
/// reducers (e.g., inside `SUM(x[*])`), fixed-index references
/// (e.g., `x[NYC]`), and dynamic-index references (e.g., `x[i+1]` where
/// `i` is a position iterator). The shape determines element-edge
/// emission and per-reference partial-equation construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RefShape {
    /// `Expr2::Var(source, ...)` — bare variable reference. In an A2A
    /// context with an arrayed source, this is same-element. In a scalar
    /// context with a scalar source, this is a plain scalar dep.
    Bare,
    /// `Expr2::Subscript(source, [literal_elem_or_int_lit, ...])` —
    /// every index is a literal element name or integer literal. The
    /// `Vec<String>` carries the resolved element names per dimension
    /// in source order (canonical lowercase).
    FixedIndex(Vec<String>),
    /// `Expr2::Subscript(source, indices)` where at least one index is
    /// `IndexExpr2::Wildcard`. Conservative full cross-product.
    Wildcard,
    /// `Expr2::Subscript(source, indices)` where at least one index is
    /// a non-literal expression (`@N`, `Range`, `StarRange`, or
    /// arbitrary `Expr`). Conservative full cross-product.
    DynamicIndex,
}

/// One occurrence of a source variable in a target's AST.
#[derive(Debug, Clone)]
pub(crate) struct ReferenceSite {
    pub source: String,
    pub shape: RefShape,
}
```

Move Phase 1's `RefShape` stub here, removing the duplicate and updating any test imports.

**Verification:**
Run: `cargo build -p simlin-engine`. Expected: compiles cleanly.

Run: `cargo test -p simlin-engine --lib`. Expected: Phase 1's ignored tests still compile and remain ignored; nothing else breaks.

**Commit:** `engine: promote RefShape to its production form with ReferenceSite`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: AST walker — `collect_reference_sites`

**Verifies:** infrastructure for AC1.1, AC1.2, AC1.3, AC1.5

**Files:**
- Modify: `src/simlin-engine/src/db_analysis.rs` (add new function before `model_element_causal_edges`)

**Implementation:**
Add a private function `collect_reference_sites` that walks a target variable's AST and returns `Vec<ReferenceSite>`. Mirror the recursion pattern of `classify_in_expr` (`db_analysis.rs:183-282`) but instead of accumulating one classification, push one `ReferenceSite` per matching occurrence.

Signature:
```rust
fn collect_reference_sites(
    target_var: &crate::variable::Variable,
    source_ident: &str,
    source_is_arrayed: bool,
) -> Vec<ReferenceSite>
```

Recursion rules (per Expr2 variant):
- `Const(..)`: no-op.
- `Var(ident, array_bounds, ..)`: if `ident == source_ident`, push `ReferenceSite { source, shape: RefShape::Bare }`. (Whether the source is arrayed or scalar is encoded by the caller's downstream edge-emission, not by the shape itself — Bare carries both same-element and scalar-dep semantics, which we resolve at edge-emission time.)
- `Subscript(ident, indices, ..)`:
  - If `ident == source_ident`: classify shape from indices. Apply rules:
    - any `IndexExpr2::Wildcard(_)` → `Wildcard`
    - all indices are `IndexExpr2::Expr(Expr2::Const(...))` where the const is a literal element name (lookup against the source's dimensions) or an integer literal → `FixedIndex(vec![name1, name2, ...])`
    - any other index pattern (`StarRange`, `DimPosition`, `Range`, non-const `Expr`) → `DynamicIndex`
  - Push the appropriate `ReferenceSite`. **Also recurse into each index expression** to handle nested references like `source_outer[source_inner[*]]`.
  - If `ident != source_ident`: still recurse into each `IndexExpr2::Expr(e)` and `IndexExpr2::Range(l, r, _)` for nested refs.
- `App(builtin, ..)`: walk via `walk_builtin_expr` with `BuiltinContents`:
  - `BuiltinContents::Ident(id, _)`: if `id == source_ident`, push `ReferenceSite { shape: Bare }`.
  - `BuiltinContents::Expr(sub_expr)`: recurse into sub_expr.
- `Op1(_, operand, ..)`: recurse into operand.
- `Op2(_, lhs, rhs, ..)`: recurse into both.
- `If(cond, then, else, ..)`: recurse into all three.

Return order is the AST-walk order. Duplicate sites with identical `(source, shape)` are kept (caller can dedupe; the count may matter for downstream metrics).

**Helper for literal-element classification:**
Add a small helper `fn resolve_literal_index(idx: &IndexExpr2, source_dims: &[Dimension]) -> Option<String>` that:
- Matches `IndexExpr2::Expr(Expr2::Const(s, _, _))` and returns `Some(canonicalize(&s).into_owned())` if the canonical form matches one of `source_dims[0]`'s element names, else None.
- Returns None for any other `IndexExpr2` variant.

For multidimensional subscripts, all indices must resolve via `resolve_literal_index` for the result to be `FixedIndex`. If any one fails, the subscript is `DynamicIndex` (or `Wildcard` if a wildcard is present).

The walker takes the source variable's dimensions as a parameter so it can validate literal subscripts against the source's actual element list. If a "literal" doesn't match a known element name (typo, unresolved subscript), classify defensively as `DynamicIndex`.

Refined signature:
```rust
fn collect_reference_sites(
    target_var: &crate::variable::Variable,
    source_ident: &str,
    source_is_arrayed: bool,
    source_dims: &[crate::dimensions::Dimension],
) -> Vec<ReferenceSite>
```

**Testing:**
Add `#[cfg(test)] mod collect_reference_sites_tests` in `db_analysis.rs`. Mirror the existing `classify_element_dependency_tests` pattern but assert on `Vec<ReferenceSite>` shape:

```rust
#[test]
fn ref_site_bare_a2a() {
    let project = TestProject::new("bare_a2a")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("population[Region]", "100")
        .array_aux("births[Region]", "population * 0.1");
    let sites = collect(&project, "births", "population");
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].shape, RefShape::Bare);
}

#[test]
fn ref_site_fixed_index() {
    let project = TestProject::new("fixed")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("population[Region]", "100")
        .array_aux("relative_pop[Region]", "population / population[NYC]");
    let sites = collect(&project, "relative_pop", "population");
    assert_eq!(sites.len(), 2);
    // First occurrence is the bare numerator.
    assert_eq!(sites[0].shape, RefShape::Bare);
    // Second is the fixed-index denominator.
    assert_eq!(sites[1].shape, RefShape::FixedIndex(vec!["nyc".to_string()]));
}

#[test]
fn ref_site_wildcard_reducer() {
    let project = TestProject::new("wild")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("population[Region]", "100")
        .scalar_aux("total", "SUM(population[*])");
    let sites = collect(&project, "total", "population");
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].shape, RefShape::Wildcard);
}

#[test]
fn ref_site_mixed_bare_and_wildcard() {
    let project = TestProject::new("mixed")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("population[Region]", "100")
        .array_aux("share[Region]", "population / SUM(population[*])");
    let sites = collect(&project, "share", "population");
    assert_eq!(sites.len(), 2);
    let shapes: Vec<&RefShape> = sites.iter().map(|s| &s.shape).collect();
    assert!(shapes.contains(&&RefShape::Bare));
    assert!(shapes.contains(&&RefShape::Wildcard));
}
```

These tests run as part of the regular suite (no `#[ignore]`).

**Verification:**
Run: `cargo test -p simlin-engine --lib collect_reference_sites_tests`. Expected: all pass.

**Commit:** `engine: AST walker collect_reference_sites with shape classification`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Edge-emission helper — `emit_edges_for_reference`

**Verifies:** infrastructure for AC1.1, AC1.2, AC1.3, AC1.5

**Files:**
- Modify: `src/simlin-engine/src/db_analysis.rs`

**Implementation:**
Add a private function that, given source/target dimensions, a `RefShape`, and the existing `element_edges: &mut HashMap<String, BTreeSet<String>>`, emits the correct element edges for one reference site. This replaces the role of `expand_edge_to_elements`.

```rust
fn emit_edges_for_reference(
    from_name: &str,
    to_name: &str,
    from_dims: &[crate::dimensions::Dimension],
    to_dims: &[crate::dimensions::Dimension],
    shape: &RefShape,
    element_edges: &mut HashMap<String, BTreeSet<String>>,
)
```

Cases to handle (matching the truth table in the design plan):

| `from_dims` | `to_dims` | `shape` | Edges emitted |
|------------|-----------|---------|---------------|
| `[]` | `[]` | `Bare` | `from -> to` |
| `[]` | non-empty | `Bare` | `from -> to[d]` for each d in cartesian(`to_dims`) |
| non-empty | `[]` | `Bare` | `from[d] -> to` for each d in cartesian(`from_dims`) |
| non-empty | non-empty (same dims) | `Bare` | `from[d] -> to[d]` per shared element |
| non-empty | non-empty (partial collapse) | `Bare` | `from[d1,d2] -> to[d1]` (delegate to existing `expand_same_element`) |
| non-empty | any | `Wildcard` or `DynamicIndex` | full cross product (same as today's `CrossElement` branch) |
| non-empty | `[]` | `FixedIndex(elem)` | `from[elem] -> to` (one edge) |
| non-empty | non-empty | `FixedIndex(elem)` | `from[elem] -> to[d]` for each d in cartesian(`to_dims`) |

For multi-dim FixedIndex with all indices resolved (`FixedIndex(vec!["NYC", "Adult"])`), emit `from[nyc,adult] -> to[d]` for each target d. For multi-dim partial fixed (handled as `DynamicIndex` per the conservative rule in Task 2), use the full cross product.

Reuse `cartesian_element_names`, `format_element_name`, `format_multi_element_name`, and `expand_same_element` (still present in `db_analysis.rs`).

**Testing:**
Direct unit tests are not strictly necessary — the function is exercised by the integration through `model_element_causal_edges` (Tasks 4 and 5). However, a small private smoke test helps catch regressions in helper composition. Add 2–3 #[test] cases in `mod emit_edges_for_reference_tests`:

```rust
#[test]
fn fixed_index_to_arrayed_target() {
    let region = make_named_dimension("Region", &["NYC", "Boston"]);
    let mut edges = HashMap::new();
    emit_edges_for_reference(
        "pop",
        "rel",
        &[region.clone()],
        &[region.clone()],
        &RefShape::FixedIndex(vec!["nyc".to_string()]),
        &mut edges,
    );
    let from = edges.get("pop[nyc]").expect("from key");
    assert!(from.contains("rel[nyc]"));
    assert!(from.contains("rel[boston]"));
    assert_eq!(from.len(), 2);
    assert!(edges.get("pop[boston]").is_none());
}
```

(Reuse `make_named_dimension` from `ltm_augment.rs::tests` if visible; otherwise inline a small dimension constructor.)

**Verification:**
Run: `cargo test -p simlin-engine --lib emit_edges_for_reference_tests`. Expected: all pass.

**Commit:** `engine: edge-emission helper emit_edges_for_reference`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-7) -->

<!-- START_TASK_4 -->
### Task 4: Pivot `model_element_causal_edges` to use the new walker

**Verifies:** ltm-per-ref-elem-graph.AC1.1, AC1.2, AC1.3, AC1.5 (turns Phase 1's red tests green)

**Files:**
- Modify: `src/simlin-engine/src/db_analysis.rs:925-1053` (the `model_element_causal_edges` body)

**Implementation:**
Replace the inner edge-classification loop in `model_element_causal_edges` (lines 985–1033) with the new walker. The structural flow→stock SameElement special-case and the stock-name expansion pass remain unchanged.

Replacement logic (pseudo-code):
```rust
// (Existing code: build structural_flow_to_stock set, lines 970-982 — unchanged.)

for (from_name, to_set) in &variable_edges.edges {
    let from_dims = lookup_dims(from_name, &mut dim_cache);
    for to_name in to_set {
        let to_dims = lookup_dims(to_name, &mut dim_cache);

        // Fast path: scalar -> scalar pass-through.
        if from_dims.is_empty() && to_dims.is_empty() {
            element_edges
                .entry(from_name.clone())
                .or_default()
                .insert(to_name.clone());
            continue;
        }

        // Structural flow->stock edges: emit same-element diagonal
        // and skip AST analysis (stock equations don't reference flows).
        if structural_flow_to_stock.contains(&(from_name.clone(), to_name.clone()))
            && !from_dims.is_empty()
            && !to_dims.is_empty()
        {
            emit_edges_for_reference(
                from_name,
                to_name,
                &from_dims,
                &to_dims,
                &RefShape::Bare,
                &mut element_edges,
            );
            continue;
        }

        // AST-based emission: collect reference sites and emit edges per site.
        let target_var = match reconstruct_single_variable(db, model, project, to_name) {
            Some(v) => v,
            None => {
                // Couldn't reconstruct (shouldn't happen for well-formed models);
                // fall back to scalar broadcast emission as today did.
                emit_edges_for_reference(
                    from_name,
                    to_name,
                    &from_dims,
                    &to_dims,
                    &RefShape::Bare,
                    &mut element_edges,
                );
                continue;
            }
        };
        let source_is_arrayed = !from_dims.is_empty();
        let sites = collect_reference_sites(
            &target_var,
            from_name,
            source_is_arrayed,
            &from_dims,
        );

        if sites.is_empty() {
            // Defensive: no AST reference found but the variable-level edge
            // exists. Fall back to scalar broadcast (same as today's
            // ElementDependencyKind::Scalar path).
            emit_edges_for_reference(
                from_name,
                to_name,
                &from_dims,
                &to_dims,
                &RefShape::Bare,
                &mut element_edges,
            );
            continue;
        }

        for site in sites {
            emit_edges_for_reference(
                from_name,
                to_name,
                &from_dims,
                &to_dims,
                &site.shape,
                &mut element_edges,
            );
        }
    }
}

// (Existing code: expand stocks, lines 1037-1047 — unchanged.)
```

The `lookup_dims` closure and `dim_cache` are kept for performance.

**Testing:**
Tests now run live, not ignored. Phase 1's Tasks 1, 2, and 3 (and Task 7 of Phase 1, the hygiene check) — un-`#[ignore]` them in this commit:

- `db_element_graph_tests.rs` Phase-1 Tasks 1's three #[test] functions (fixed-index broadcast, wildcard reducer, multidim partial-fixed)
- `tests/simulate_ltm.rs` Phase-1 Task 2's `test_cross_element_ltm_edge_set_truthful`
- `db_element_graph_proptest.rs` Phase-1 Task 3's projection invariant

All five should now pass. Existing tests in `db_element_graph_tests.rs` (the ones that use Wildcard patterns) must continue to pass without modification — they rely on Wildcard's full-cross emission, which the new walker also produces.

**Verification:**
Run: `cargo test -p simlin-engine --test db_element_graph_proptest`. Expected: passes.
Run: `cargo test -p simlin-engine element_graph`. Expected: all formerly-ignored tests pass; existing tests pass.
Run: `cargo test -p simlin-engine --test simulate_ltm test_cross_element_ltm`. Expected: existing tests pass; new edge-set test passes.

If any test fails, do NOT proceed to Task 5. Diagnose and fix before continuing — likely culprits are: (a) shape classification defaulting wrong for an unhandled AST pattern, (b) edge-emission rule producing wrong subscript names (case-sensitivity), (c) missed structural flow→stock bypass.

**Commit:** `engine: pivot model_element_causal_edges to AST-walking per-reference emission`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Run full simulate_ltm suite, document any golden-data shifts

**Verifies:** ltm-per-ref-elem-graph.AC4.3 (interim — full Phase 4 coverage comes later)

**Files:**
- None modified (or test fixtures updated with documented justification)

**Implementation:**
Run the full `cargo test -p simlin-engine` suite (workspace test budget 180s). The goal is to surface any test that depends on the spurious-edge behavior. Per Phase 2 codebase investigation, no LTM test has fragile NxN-edge-count assertions, but the population/cross-element/arrayed integration tests should be re-validated.

Specifically run: `cargo test -p simlin-engine --test simulate_ltm 2>&1 | tee /tmp/phase2-simulate-ltm.log`.

Expected outcomes:
- `simulates_population_ltm`: scalar model, no fixed-index, **must pass without changes**.
- `discovery_logistic_growth_finds_both_loops`: scalar, **must pass**.
- `discovery_arms_race_3party`: scalar arms-race, **must pass**.
- `discovery_decoupled_stocks`: scalar, **must pass**.
- `test_cross_element_ltm_exhaustive` and `_discovery`: arrayed with FixedIndex; loop counts and per-loop scores may change. Read the log carefully and document any deltas.
- `test_arrayed_population_ltm_exhaustive`: arrayed; the existing test was relaxed to "at least one slot non-zero" per tech-debt #34's resolution note. After this phase, more slots may be legitimately non-zero. Document.

For each test that has a deltas-from-expected:
1. Investigate manually whether the new value is correct (the new walker's edges match truth).
2. If correct, update the golden assertion AND add a comment in the test linking to this implementation phase explaining the change.
3. If incorrect, diagnose the walker bug and return to Task 4.

If updating any test: include the test name and the before/after values in the commit message.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate_ltm`. Expected: all pass.

If golden updates were made, run the pre-commit hook to confirm the workspace stays under 180s and no other test regresses.

**Commit:** `engine: re-validate simulate_ltm against new edge structure` (with concrete test-by-test deltas in the body)
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Delete obsolete classification code

**Verifies:** internal cleanup; ensures the design's "delete `ElementDependencyKind` and unused expansion helpers" instruction lands

**Files:**
- Modify: `src/simlin-engine/src/db_analysis.rs`

**Implementation:**
Remove all of:
- `enum ElementDependencyKind` (line 78)
- `fn classify_element_dependency` (lines 117-170)
- `fn classify_in_expr` (lines 183-282)
- `fn expand_edge_to_elements` (lines 309-401)
- `mod classify_element_dependency_tests` (lines 1400-1553)

**Keep:**
- `cartesian_element_names` (line 411) — used by new walker
- `expand_same_element` (line 454) — used by new walker for partial-collapse cases
- `format_element_name` and `format_multi_element_name` (lines 59-67) — used by new walker
- The `dimension_element_names` thin wrapper at line 287 (delegates to `ltm_augment::dimension_element_names`) — still useful

After deletion, verify the file compiles:
- `cargo check -p simlin-engine`
- Address any newly-orphaned imports or `use` lines.

The codebase-investigator confirmed `ElementDependencyKind` is private to `db_analysis.rs` — no other module needs adjustment.

**Testing:**
After the deletion, run the full Rust test suite to confirm no orphan reference exists:
- `cargo test -p simlin-engine --lib`
- `cargo clippy -p simlin-engine`

**Verification:**
Run: `cargo build -p simlin-engine`. Expected: clean build.
Run: `cargo test -p simlin-engine`. Expected: all tests pass.
Run: `cargo clippy -p simlin-engine -- -D warnings`. Expected: no clippy errors.

**Commit:** `engine: remove ElementDependencyKind in favor of per-reference walker`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Phase 2 wrap-up — pre-commit hook end-to-end

**Verifies:** ltm-per-ref-elem-graph.AC5.2 (interim — pre-commit budget honored)

**Files:**
- None modified

**Implementation:**
Trigger the pre-commit hook by attempting to commit a no-op (or by amending the most recent commit's message). Confirm the full hook completes within budget:

```bash
# Sanity: no uncommitted changes.
git status
# Trigger pre-commit by amending HEAD message (no content change).
git commit --amend --no-edit
```

Watch the hook output for:
- Rust fmt: clean
- Rust clippy: clean
- Rust test (`cargo test --workspace` under 180s with timeout-30s SIGKILL margin)
- TypeScript lint/build/tsc/test
- WASM build
- Python (pysimlin) tests

If any stage fails, fix the root cause — do not skip with `--no-verify`.

**Verification:**
Run: `git commit --amend --no-edit`. Expected: pre-commit prints "All pre-commit checks passed!" and the amend completes.

**Commit:** No new commit (this is a verification gate). If anything was fixed, a separate commit captures the fix.
<!-- END_TASK_7 -->

<!-- END_SUBCOMPONENT_B -->

---

## Phase Done When

- All 6 implementation tasks (Tasks 1–6) committed; Task 7 verification gate passes.
- `model_element_causal_edges` produces the truthful per-reference edge set described in the design plan's truth table.
- All Phase 1 red tests are now green and have their `#[ignore]` annotations removed.
- All existing tests in `db_element_graph_tests.rs`, `db_ltm_tests.rs`, `db_ltm_unified_tests.rs`, `db_ltm_module_tests.rs` continue to pass without modification.
- `tests/simulate_ltm.rs` passes end-to-end. Any test whose assertion was tightened, loosened, or value-shifted has a comment referencing this phase.
- `ElementDependencyKind`, `classify_element_dependency`, `classify_in_expr`, `expand_edge_to_elements`, and `mod classify_element_dependency_tests` are deleted from `db_analysis.rs`.
- Pre-commit hook passes cleanly within the 180s budget.
