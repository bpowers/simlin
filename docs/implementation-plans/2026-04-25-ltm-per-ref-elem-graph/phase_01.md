# Phase 1 — TDD Red-Phase Tests

**Goal:** Write tests that pin the desired post-refactor behavior of the LTM element-level causal graph and per-shape partial equations. Every test in this phase must be **red** today (fail or assert an unintended outcome) and must turn green after Phase 2 + Phase 3 land.

**Architecture:** Add edge-set unit tests, a proptest invariant cross-validating variable-level vs. element-level edges, partial-equation unit tests in `ltm_augment.rs`, and pin existing integration tests with explicit baselines. Phase 1 introduces *minimal production scaffolding* — a `RefShape` enum stub and a `build_partial_equation_for_shape` stub function in `src/simlin-engine/src/ltm_augment.rs` — solely so the new tests compile. The stubs are gated behind `#[allow(dead_code)]` until Phase 2/3 populate their use-sites and remove the allow attribute.

**Tech Stack:** Rust, salsa-tracked `model_element_causal_edges` and `model_causal_edges`, `TestProject` builder, `proptest` (existing dep), Expr0 AST for partial-equation tests.

**Scope:** 7 phases total; this is Phase 1.

**Codebase verified:** 2026-04-25 (codebase-investigator dispatched and reported file locations, builder API, test conventions, and existing assertion styles).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### ltm-per-ref-elem-graph.AC1: Element-edge structure is per-AST-reference
- **ltm-per-ref-elem-graph.AC1.1 Fixed-index broadcast not over-expanded:** For `relative_pop[R] = population / population[NYC]` over a dimension R of size N, the element-graph must contain exactly the diagonal same-element edges (`population[d] -> relative_pop[d]` for each d in R) plus the broadcast-from-NYC edges (`population[NYC] -> relative_pop[d]` for each d in R, deduplicated). Total unique edges: 2N − 1, not N².
- **ltm-per-ref-elem-graph.AC1.2 Wildcard reducer remains all-pairs:** For `share[R] = population / SUM(population[*])`, the wildcard reducer continues to emit all-pairs edges from every source element to every target element (`population[d] -> share[e]` for all d, e), *in addition to* the diagonal SameElement edges. The bare-Var numerator's diagonal edges must not be lost.
- **ltm-per-ref-elem-graph.AC1.3 Cross-element fixture edge set:** For `test/cross_element_ltm/cross_element.stmx`, the element graph must contain the truthful broadcast edges for every fixed-index reference (`population[NYC] -> migration_pressure[d]`, `migration_pressure[Boston] -> migration_in[NYC]`, etc.) and must NOT contain the spurious all-pairs edges that today's code produces.
- **ltm-per-ref-elem-graph.AC1.4 Variable-level projection invariant:** For every project, the set of variable-level edges produced by stripping subscripts and deduplicating from `model_element_causal_edges` equals the set produced by `model_causal_edges`. Verified with property tests over randomly generated arrayed models.
- **ltm-per-ref-elem-graph.AC1.5 Multidim partial-fixed conservative:** For multidimensional sources where some indices are literal and others are wildcards (e.g., `source[NYC, *]`), the conservative initial behavior is to treat as Wildcard shape. Documented with a TODO; not a regression vs today.

### ltm-per-ref-elem-graph.AC2: Per-shape partial equations are correct
- **ltm-per-ref-elem-graph.AC2.1 Bare-shape partial holds wildcard at PREVIOUS:** For `share[R] = population / SUM(population[*])` and the link score keyed by `(population, share, Bare)`, the partial equation must leave the bare `population` reference live and wrap the `population[*]` inside the SUM in `PREVIOUS()`. The resulting link-score magnitude is partition-aware and not pinned at 1.
- **ltm-per-ref-elem-graph.AC2.2 Wildcard-shape partial holds bare at PREVIOUS:** For the same equation, the link score keyed by `(population, share, Wildcard)` must wrap the bare `population` in `PREVIOUS()` and leave the wildcard reducer live.
- **ltm-per-ref-elem-graph.AC2.3 FixedIndex per-element partials:** For `migration_pressure[NYC] = (pop[NYC] - pop[Boston]) * 0.01`, the link score keyed by `(pop, migration_pressure, FixedIndex(NYC))` must yield partial `(pop[NYC] - PREVIOUS(pop[Boston])) * 0.01`. The link score keyed by `(pop, migration_pressure, FixedIndex(Boston))` must yield partial `(PREVIOUS(pop[NYC]) - pop[Boston]) * 0.01`.

> Note: AC2.4 (other-source refs still wrapped) is exercised indirectly by AC2.1–2.3 because each test asserts on full partial-equation strings that include other-source references. A standalone AC2.4 assertion appears in Phase 3.

---

## Implementation Tasks

The strategy is: write the tests, run them, and confirm they fail in the expected ways. Failures may take several forms (assertion failure, panic from a not-yet-existing API, compile error if signatures must extend). Phase 1 commits the test code plus the minimal production stubs needed for tests to compile; Phase 2 + Phase 3 populate the APIs and logic that turn each test green.

For tests that assert behavior of APIs that **do not yet exist** (e.g., a `RefShape`-aware `build_partial_equation`), put the test behind `#[ignore]` with a comment pointing to the phase that activates it. This keeps Phase 1 commits green under pre-commit while still pinning the contract.

For tests that assert behavior of APIs that **exist today but produce the wrong result**, use `#[ignore]` with the same convention: they pass at Phase 2/3 boundary and have the `#[ignore]` removed.

This convention preserves the pre-commit budget (under 180s) — ignored tests are skipped during normal `cargo test` and run on demand with `--ignored`.

<!-- START_TASK_0 -->
### Task 0: Production scaffolding — `RefShape` enum and `build_partial_equation_for_shape` stub

**Verifies:** none directly (infrastructure for AC2.1, AC2.2, AC2.3 — required by Tasks 4–6 below)

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` (add near the top of the file, before `wrap_deps_in_previous`)

**Implementation:**
Add the minimal production code that subsequent Phase 1 tests depend on. Without these stubs, Phase 1's Tasks 4–6 (which reference `RefShape` and a shape-aware partial-equation builder) cannot compile.

```rust
/// Access shape of a single AST reference site to a source variable.
///
/// Phase 1 introduces this as a stub for the test scaffolding; Phases 2
/// and 3 fully populate the use-sites. The `Vec<String>` in `FixedIndex`
/// holds canonical (lowercase) element names per dimension in source order.
#[allow(dead_code)]  // populated in Phase 2/3
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RefShape {
    Bare,
    FixedIndex(Vec<String>),
    Wildcard,
    DynamicIndex,
}

/// Stub for the per-shape partial equation builder. Phase 3 replaces the
/// body with the real implementation. Until then, calling this panics —
/// Phase 1's tests that exercise it are `#[ignore]`-d so the panic doesn't
/// fire during normal `cargo test`.
#[allow(dead_code)]  // populated in Phase 3
pub(crate) fn build_partial_equation_for_shape(
    _equation_text: &str,
    _deps: &HashSet<Ident<Canonical>>,
    _live_source: &Ident<Canonical>,
    _live_shape: &RefShape,
    _source_dim_elements: &[Vec<String>],
) -> String {
    unimplemented!("populated in Phase 3")
}
```

**Verification:**
Run: `cargo build -p simlin-engine`. Expected: clean build (the `#[allow(dead_code)]` attribute quiets clippy until Phase 2/3 use the new symbols).

Run: `cargo clippy -p simlin-engine --all-targets -- -D warnings`. Expected: no warnings.

**Commit:** `engine: scaffold RefShape and build_partial_equation_for_shape stubs`
<!-- END_TASK_0 -->

<!-- START_TASK_PRINTEQN_VERIFY -->
### Task 0.5: Capture canonicalized output of `print_eqn` for expected-string locking

**Verifies:** prerequisite for AC2.1, AC2.2, AC2.3 (test reliability — without this step Phase 1's red tests may fail for the wrong reason and give misleading diagnostics)

**Files:**
- None modified (one-shot reconnaissance; the temporary test added during this task is deleted before commit)

**Implementation:**
The expected partial-equation strings in Tasks 5 and 6 below depend on `print_eqn`'s exact canonicalization (capitalization of identifiers, function names like `SUM` vs `sum`, element names like `NYC` vs `nyc`, whitespace, parenthesization). Today's `print_eqn` output is undocumented in the design plan. Run a one-shot exploration before locking in the expected strings:

1. In `src/simlin-engine/src/ltm_augment.rs::tests`, add a temporary `#[test] #[ignore]` function that:
   - Parses `"population / SUM(population[*])"` into an `Expr0` AST via `Expr0::new(text, LexerType::Equation)`.
   - Builds a `HashSet<Ident<Canonical>>` containing `Ident::new("population")`.
   - Calls today's existing `wrap_deps_in_previous` (lines 25–75 of `ltm_augment.rs`) on the AST with that dep set — this wraps every `population` reference uniformly. (The result is NOT what AC2.1 wants, but it tells us how `print_eqn` formats its output.)
   - `dbg!`-prints `print_eqn(&transformed)`. Expected output is one of (lock in whichever is actually produced):
     - `PREVIOUS(population) / SUM(PREVIOUS(population[*]))` (original case preserved)
     - `previous(population) / sum(previous(population[*]))` (full lowercase)
     - some variant.
   - Repeat for `"(population[NYC] - population[Boston]) * 0.01"` with `population[Boston]` excluded from the wrap set; capture the resulting form for `[NYC]`/`[Boston]` element-name canonicalization.

2. Run: `cargo test -p simlin-engine --lib explore_print_eqn -- --ignored --nocapture`. Capture the `dbg!` output.

3. Document the captured forms inline in this phase document as a "Captured `print_eqn` output" subsection so Tasks 5 and 6 can reference them. Lock in the literal expected strings in Tasks 5/6 to match the captured form. If `print_eqn` lowercases function names (e.g., `SUM` → `sum`), Tasks 5/6's expected strings reflect that exactly.

4. Delete the temporary `explore_print_eqn` test before committing Phase 1.

**Verification:**
The captured strings reflect real `print_eqn` output. Tasks 5 and 6's assertions match those strings exactly. The Phase 1 red tests then fail for one reason ("the API is missing" or "the API returned the wrong shape"), not for "the canonicalization differs from what the plan guessed."

**Commit:** No commit (the temporary test is deleted; Phase 1 Tasks 4–6 commits include the verified strings).
<!-- END_TASK_PRINTEQN_VERIFY -->

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Edge-set tests for representative AST patterns

**Verifies:** ltm-per-ref-elem-graph.AC1.1, AC1.2, AC1.5

**Files:**
- Modify: `src/simlin-engine/src/db_element_graph_tests.rs` (add tests at end of file, before any closing `}` of an outer module)

**Implementation:**
Add three #[test] functions following the file's existing helper conventions (`element_edges(project: &TestProject) -> ElementCausalEdgesResult`, `assert_edge`, `assert_no_edge`). Each test builds a `TestProject` with explicit dimensions and arrayed variables, calls `element_edges`, then asserts the **exact** expected set of edges.

The tests should be initially marked `#[ignore = "Phase 2: AST-walking element graph builder; today emits N² edges for fixed-index refs"]`. Phase 2 removes the ignore.

The three patterns to cover:

1. **fixed-index broadcast** (AC1.1): dimension `Region = {NYC, Boston, LA}` (N=3), `population[Region] = 100`, `relative_pop[Region] = population / population[NYC]`. Expected element edges from `population` to `relative_pop`:
   - `population[nyc] -> relative_pop[nyc]` (diagonal, from bare Var)
   - `population[boston] -> relative_pop[boston]` (diagonal, from bare Var)
   - `population[la] -> relative_pop[la]` (diagonal, from bare Var)
   - `population[nyc] -> relative_pop[boston]` (broadcast from NYC)
   - `population[nyc] -> relative_pop[la]` (broadcast from NYC)

   That is 5 unique edges = 2N − 1. Use `assert_edge` for each expected edge and `assert_no_edge` for at least three spurious ones today's code emits (e.g., `population[boston] -> relative_pop[nyc]`, `population[la] -> relative_pop[nyc]`, `population[boston] -> relative_pop[la]`).

2. **wildcard reducer plus bare** (AC1.2): same dimension; `share[Region] = population / SUM(population[*])`. Expected element edges from `population` to `share`:
   - All 9 edges (3×3 cartesian product) due to the wildcard reducer
   - The 3 diagonal edges must be present even when the test runs (today's code already produces them, but the refactor must not lose them).

   Assert all 9 with `assert_edge`. There is no edge for the test to negate, but the 9-edge enumeration locks the behavior.

3. **multidim partial-fixed conservative** (AC1.5): dimensions `Region = {NYC, Boston}` and `Age = {Adult, Child}`; `pop[Region, Age] = 10`; `target[Region] = pop[NYC, Adult] + SUM(pop[NYC, *])`. Expected: at minimum, `pop[nyc, adult] -> target[d]` for each d (broadcast from the literal pair), plus `pop[nyc, adult] -> target[d]`, `pop[nyc, child] -> target[d]` for each d (from the partial-wildcard SUM). The conservative initial expansion treats partial-wildcards as full Wildcard, so all `pop[nyc, *]` elements broadcast to all `target` elements. The test should assert the literal-pair edges exist AND should assert (with a comment pointing to a follow-up tech-debt item) that the partial-wildcard expansion is conservative (over-approximating).

**Testing:**
Tests assert specific edge presence and absence. For AC1.1 the failing assertion will be one of the `assert_no_edge` calls today (today emits the spurious edges). For AC1.2 the test is initially expected to pass (today's code already produces all 9); marking it `#[ignore]` is still appropriate to preserve the post-refactor green path under one-shot-on convention. For AC1.5 the test currently emits all-NxN; the conservative-after-refactor expansion still emits all-NxN for partial-wildcards, so the test should pass post-refactor with a documented TODO.

**Verification:**
Run: `cargo test -p simlin-engine --test '*' --lib element_graph_fixed_index -- --ignored --nocapture` (after activating with #[test] and removing #[ignore], or run #[ignored] explicitly). Expected initial result: AC1.1 test fails (assertion error on absent spurious edge); AC1.2 and AC1.5 tests pass.

Run: `cargo test -p simlin-engine` (regular suite). Expected: ignored tests are skipped; the rest of the suite is unchanged.

**Commit:** `engine: red tests for per-reference element-graph edge sets`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Cross-element fixture edge-set assertion

**Verifies:** ltm-per-ref-elem-graph.AC1.3

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm.rs` (extend `test_cross_element_ltm_exhaustive` at ~line 4092, OR add a sibling `test_cross_element_ltm_edge_set` test that loads the same fixture)

**Implementation:**
Add a new integration test `test_cross_element_ltm_edge_set_truthful` that loads `test/cross_element_ltm/cross_element.stmx`, builds the element-level causal graph via `model_element_causal_edges`, and asserts a specific subset of expected edges plus a specific subset of expected absences.

The fixture's relevant equations:
```
migration_pressure[NYC] = (population[NYC] - population[Boston]) * 0.01
migration_pressure[Boston] = (population[Boston] - population[NYC]) * 0.01
migration_in[NYC] = MAX(migration_pressure[Boston] * -1, 0)
migration_in[Boston] = MAX(migration_pressure[NYC] * -1, 0)
total_population = SUM(population[*])
```

Expected element edges (use canonical lowercase element names):
- `population[nyc] -> migration_pressure[nyc]` (literal NYC ref in NYC equation, same element)
- `population[boston] -> migration_pressure[nyc]` (literal Boston ref in NYC equation, broadcast)
- `population[boston] -> migration_pressure[boston]` (literal Boston ref in Boston equation, same element)
- `population[nyc] -> migration_pressure[boston]` (literal NYC ref in Boston equation, broadcast)
- `migration_pressure[boston] -> migration_in[nyc]` (literal Boston ref in NYC migration_in)
- `migration_pressure[nyc] -> migration_in[boston]` (literal NYC ref in Boston migration_in)
- `population[nyc] -> total_population` and `population[boston] -> total_population` (wildcard reducer)
- All structural flow→stock edges (births, migration_in, migration_out → population[*])

Expected absences (today emitted, must disappear after refactor):
- `migration_pressure[nyc] -> migration_in[nyc]` — `migration_in[NYC]` references only `migration_pressure[Boston]`, not `migration_pressure[NYC]`, so this edge is spurious. Today's NxN expansion creates it.
- `migration_pressure[boston] -> migration_in[boston]` — analogous, spurious.

This test must be `#[ignore = "Phase 2: post AST-walking refactor"]` initially and removed in Phase 2.

**Testing:**
Use existing helpers in `simulate_ltm.rs` for loading and salsa setup (see test_cross_element_ltm_exhaustive for the pattern). Pull `model_element_causal_edges` directly. Use `assert!(edges.get("population[nyc]").is_some_and(|t| t.contains("migration_pressure[nyc]")))` style for presence; `assert!(!edges.get("migration_pressure[nyc]").is_some_and(|t| t.contains("migration_in[nyc]")))` for absence.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate_ltm test_cross_element_ltm_edge_set_truthful -- --ignored --nocapture`. Expected: the absence assertion fails today (the spurious edge IS present); will pass after Phase 2.

**Commit:** `engine: red test for cross_element_ltm truthful edge set`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Property test — variable-level projection invariant

**Verifies:** ltm-per-ref-elem-graph.AC1.4

**Files:**
- Create: `src/simlin-engine/src/db_element_graph_proptest.rs` (new file; declared as `#[cfg(test)] #[path = "..."] mod ...;` from `db_analysis.rs` or `lib.rs`)
- Modify: appropriate parent module to declare the new test file
- Possibly: `Cargo.toml` (verify `proptest` is a dev-dep already; the codebase has it via existing `json_proptest.rs`)

**Implementation:**
Build a small, hand-crafted strategy that generates compilable arrayed `TestProject` instances and asserts the projection invariant. **Do not generate truly random equations** — random equations introduce too much noise (parse failures, dimension errors). Instead, parameterize a fixed schema:

- Choose dimension N from `{1, 2, 3, 5}`.
- Choose 2 to 5 arrayed variables over a single dimension.
- Each variable's equation is sampled from a small bag of patterns:
  - bare same-element ref (`other_var`)
  - wildcard reducer (`SUM(other_var[*])`)
  - fixed-index ref (`other_var[<elem>]` where elem is a literal)
  - sum of two variables
  - constant
- Optionally generate one scalar variable referenced by some arrayed variable.
- Build `TestProject`, call `build_datamodel`, then call both `model_element_causal_edges` and `model_causal_edges` on the same DB.

The invariant: `{(strip_subscript(from), strip_subscript(to)) for (from, to) in element_edges} == variable_edges` (set equality after stripping and deduplicating).

Use `proptest` `with_cases(32)` (compilation per case is non-trivial; see `docs/dev/rust.md` 2s-per-test budget). Mark the test `#[ignore = "Phase 2: requires AST-walking element graph"]` initially.

**Testing:**
Single proptest assertion: the projection equals the variable-level edge set. Failure mode today: cross-element wildcard refs already match correctly (they emit per-source-per-target edges), so the **set equality** projection should hold for today's behavior on most patterns. The pattern this test catches is **edge omission** in the new walker — Phase 2 might forget to emit a class of edges. The fixed-index pattern in particular is interesting: today's NxN over-emission still projects to a single variable-level edge, so this proptest doesn't directly catch the AC1.1 over-expansion bug; that's covered by Tasks 1 and 2. This proptest is an **anti-regression** for Phase 2.

**Verification:**
Run: `cargo test -p simlin-engine --lib db_element_graph_proptest --nocapture`. Expected: passes today on all 32 cases (the projection invariant is currently satisfied). **Do not** mark this test `#[ignore]`. Run it as a regression guard from Day 1 — this catches accidental edge omission introduced by Phase 2's refactor.

If the test fails today, that's a genuine bug in the existing code that must be filed — investigate and surface to user before proceeding. (The codebase-investigator's analysis suggests the invariant currently holds, but verifying it as a precondition of Phase 2 is part of the value.)

**Commit:** `engine: proptest pinning element-graph projection invariant`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->

<!-- START_TASK_4 -->
### Task 4: Partial-equation test infrastructure

**Verifies:** sets up AC2.1, AC2.2, AC2.3 (the actual assertions live in Tasks 5 and 6)

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` `#[cfg(test)] mod tests` block (starting at ~line 872)

**Implementation:**
Task 0 already added `RefShape` and `build_partial_equation_for_shape` stubs to `ltm_augment.rs`. Task 0.5 captured `print_eqn`'s canonicalization. Tasks 5 and 6 below call `build_partial_equation_for_shape`; Task 4 just adds the per-test helper.

Add a small test fixture helper inside the existing `#[cfg(test)] mod tests`:

```rust
fn deps_set(idents: &[&str]) -> HashSet<Ident<Canonical>> {
    idents.iter().map(|s| Ident::new(s)).collect()
}
```

(`use crate::common::Ident;` and `use std::collections::HashSet;` are already in scope from the existing test module.)

The Phase 1 tests in Tasks 5 and 6 call `build_partial_equation_for_shape` from Task 0's stub. The stub panics with `unimplemented!()`, so the tests are `#[ignore = "Phase 3: per-shape partial equations"]` until Phase 3 replaces the body.

**Testing:**
This task does not run tests directly; it sets up the helpers and module imports. Verify the file still compiles:

Run: `cargo test -p simlin-engine --lib --no-run`. Expected: success.

**Commit:** `engine: scaffold per-shape partial-equation test fixture`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Bare-shape and Wildcard-shape partial equation tests

**Verifies:** ltm-per-ref-elem-graph.AC2.1, AC2.2

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` `#[cfg(test)] mod tests` block

**Implementation:**
Two #[test] functions, both `#[ignore = "Phase 3: per-shape partial equations"]`:

1. `test_partial_equation_share_bare_shape`: equation text `"population / SUM(population[*])"`, deps `{population}`, source `population`, shape `RefShape::Bare`. Expected partial: `population / PREVIOUS(SUM(population[*]))` (the bare ref is live; the wildcard is wrapped). Use `assert_eq!(partial, "population / PREVIOUS(SUM(population[*]))")` with a leading-comment note that whitespace is parser-canonicalized via `print_eqn`.

2. `test_partial_equation_share_wildcard_shape`: same equation and deps, source `population`, shape `RefShape::Wildcard`. Expected partial: `PREVIOUS(population) / SUM(population[*])` (the bare is wrapped; the wildcard is live).

Both tests stay `#[ignore]` until Phase 3.

> **Locking expected strings:** Use the canonicalization captured in Task 0.5 to lock in the exact expected strings. If `print_eqn` lowercases function names (e.g., `SUM` → `sum`), use the lowercase form. The strings shown above assume original-case preservation — adjust if Task 0.5's reconnaissance shows otherwise.

**Testing:**
`build_partial_equation_for_shape` exists from Task 0 (as a stub). The tests run by calling it directly; today's stub panics with `unimplemented!()`, so each test fails with a clear panic message that documents the desired post-Phase-3 contract.

**Verification:**
Run: `cargo test -p simlin-engine --lib build_partial_equation -- --ignored`. Expected: both tests fail (the `build_partial_equation_for_shape` API is a stub that returns the empty string, or panics with `unimplemented!()`).

Run: `cargo test -p simlin-engine`. Expected: rest of suite passes; ignored tests skipped.

**Commit:** `engine: red tests for per-shape partial equations (Bare/Wildcard)`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: FixedIndex partial equation tests

**Verifies:** ltm-per-ref-elem-graph.AC2.3

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` `#[cfg(test)] mod tests` block

**Implementation:**
Two #[test] functions, both `#[ignore = "Phase 3: per-shape partial equations"]`:

1. `test_partial_equation_migration_pressure_fixed_nyc`: equation text `"(population[NYC] - population[Boston]) * 0.01"`, deps `{population}`, source `population`, shape `RefShape::FixedIndex(vec!["NYC".to_string()])`. Expected partial: `(population[NYC] - PREVIOUS(population[Boston])) * 0.01`.

2. `test_partial_equation_migration_pressure_fixed_boston`: same equation, source `population`, shape `RefShape::FixedIndex(vec!["Boston".to_string()])`. Expected partial: `(PREVIOUS(population[NYC]) - population[Boston]) * 0.01`.

> **Element-name canonicalization:** Task 0.5 captured the actual `print_eqn` output for `(population[NYC] - PREVIOUS(population[Boston])) * 0.01`. Lock in the captured form (preserves original case OR lowercases — whichever Task 0.5 documented). Match the FixedIndex variant's `Vec<String>` to the captured form too: if `print_eqn` lowercases element names, use `RefShape::FixedIndex(vec!["nyc".to_string()])` not `"NYC"`.

Both tests stay `#[ignore]` until Phase 3.

**Testing:**
Same as Task 5 — these tests fail today because the API is a stub.

**Verification:**
Run: `cargo test -p simlin-engine --lib partial_equation_migration -- --ignored`. Expected: both tests fail.

**Commit:** `engine: red tests for FixedIndex per-shape partial equations`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_7 -->
### Task 7: Pre-commit and cargo test hygiene check

**Verifies:** none directly (infrastructure)

**Files:**
- None modified

**Implementation:**
After Tasks 0–6 are committed, run the pre-commit hook end-to-end and verify:
1. All ignored tests are properly ignored (none accidentally run during `cargo test`).
2. The workspace test cap (180s) is not exceeded.
3. Clippy lints stay clean — `dead_code` warnings on the new `RefShape` variants and `build_partial_equation_for_shape` stub are expected to be silenced by Task 0's `#[allow(dead_code)]` attributes; remove the attribute in Phase 2/3 when the symbols become reachable.

**Verification:**

Run: `cargo test -p simlin-engine 2>&1 | tail -20`. Expected: shows `N ignored` at the end with the count matching the number of `#[ignore]` annotations added in Tasks 4–6 (about 6–7 ignored tests; Task 3's proptest is NOT ignored).

Run: `bash scripts/pre-commit`. Expected: prints "All pre-commit checks passed!" within budget. (Run the hook script directly rather than synthesizing an empty commit.)

**Commit:** No commit (this task is a verification gate).
<!-- END_TASK_7 -->

---

## Phase Done When

- All 8 implementation tasks (Task 0, Task 0.5, Tasks 1–6) committed; Task 7 verification gate passes.
- Each test added is either passing (locking in current correct behavior, e.g., AC1.2 wildcard reducer; AC1.4 projection invariant) or `#[ignore]`-marked with a comment pointing to the phase that activates it.
- `cargo test -p simlin-engine` passes within the 180s budget.
- Running `cargo test -p simlin-engine -- --ignored` shows the expected red tests, each with a clear failure message that documents the desired post-refactor behavior.
- The only production code added is the `RefShape` enum stub and `build_partial_equation_for_shape` stub from Task 0, both gated with `#[allow(dead_code)]`.
- `print_eqn` canonicalization captured by Task 0.5 is documented inline; Tasks 5/6 expected strings match the captured form exactly.
- Pre-commit hook passes.
