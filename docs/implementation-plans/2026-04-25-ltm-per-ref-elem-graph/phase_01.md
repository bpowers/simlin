# Phase 1 — TDD Red-Phase Tests

**Goal:** Write tests that pin the desired post-refactor behavior of the LTM element-level causal graph and per-shape partial equations. Every test in this phase must be **red** today (fail or assert an unintended outcome) and must turn green after Phase 2 + Phase 3 land.

**Architecture:** Add edge-set unit tests, a proptest invariant cross-validating variable-level vs. element-level edges, partial-equation unit tests in `ltm_augment.rs`, and pin existing integration tests with explicit baselines. No production code changes.

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

The strategy is: write the tests, run them, and confirm they fail in the expected ways. Failures may take several forms (assertion failure, panic from a not-yet-existing API, compile error if signatures must extend). Phase 1 only commits the test code; Phase 2 + Phase 3 will introduce APIs and logic that turn each test green.

For tests that assert behavior of APIs that **do not yet exist** (e.g., a `RefShape`-aware `build_partial_equation`), put the test behind `#[ignore]` with a comment pointing to the phase that activates it. This keeps Phase 1 commits green under pre-commit while still pinning the contract.

For tests that assert behavior of APIs that **exist today but produce the wrong result**, use `#[ignore]` with the same convention: they pass at Phase 2/3 boundary and have the `#[ignore]` removed.

This convention preserves the pre-commit budget (under 180s) — ignored tests are skipped during normal `cargo test` and run on demand with `--ignored`.

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
Run: `cargo test -p simlin-engine --lib db_element_graph_proptest -- --ignored --nocapture`. Expected: passes today on all 32 cases (the projection invariant is currently satisfied). The ignore is removed in Phase 2 to lock it in as a regression guard.

If the test fails today, that's a genuine bug in the existing code that must be filed — investigate and surface to user before proceeding.

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
Today's `build_partial_equation` (lines 92–113) takes `(equation_text: &str, deps: &HashSet<Ident<Canonical>>, exclude: &Ident<Canonical>)`. Phase 3 will introduce a new entry point that takes an additional `RefShape` parameter (or a `(source, RefShape)` tuple) and produces a per-shape partial. Phase 1's job is to **document the desired contract** via tests — the Phase 3 implementation will adjust signatures so these tests compile and pass.

Add a small test fixture helper:

```rust
fn deps_set(idents: &[&str]) -> HashSet<Ident<Canonical>> {
    idents.iter().map(|s| Ident::new(s)).collect()
}
```

(Adapt to existing imports — `use crate::common::Ident;` already in scope.)

Then add three test functions (Tasks 5 and 6 below) that call a not-yet-existing `build_partial_equation_for_shape(equation_text, deps, source, ref_shape)` function. Mark each test `#[ignore = "Phase 3: per-shape partial equations"]` and add a `#[allow(dead_code)]` to any helper that becomes unused if the new API isn't built yet.

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

**Testing:**
The expected output strings depend on `print_eqn`'s formatting. To validate the expected strings before committing, the test author may temporarily add a `dbg!(partial)` call and run with `--ignored --nocapture` to see what the new builder produces. Once verified, lock in the expected string.

These tests both must call a `RefShape` enum that doesn't exist yet. Phase 1's commit therefore introduces a public-or-internal `RefShape` enum stub in `ltm_augment.rs` (or in a new `crate::ltm::ref_shape` module) just sufficient for the tests to compile. The variants needed: `Bare`, `Wildcard`, `FixedIndex(Vec<String>)`, `DynamicIndex`. Phase 3 will populate the use-sites that produce values of this type.

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

Element-name canonicalization may apply — verify with `dbg!` whether the live string uses original-case `NYC` or canonical `nyc`. Today's `print_eqn` typically preserves the original form. Lock in whichever the new builder produces.

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
After Tasks 1–6 are committed, run the pre-commit hook end-to-end and verify:
1. All ignored tests are properly ignored (none accidentally run during `cargo test`).
2. The workspace test cap (180s) is not exceeded.
3. Clippy lints stay clean — particularly `dead_code` if the new RefShape enum has unused variants today.

Use `#[allow(dead_code)]` sparingly — only on the RefShape variants that genuinely have no compile-time use yet. Phase 3 removes the allow.

**Verification:**

Run: `cargo test -p simlin-engine 2>&1 | tail -20`. Expected: shows `N ignored` at the end with the count matching the number of `#[ignore]` annotations added in Tasks 1–6 (about 7–8 ignored tests).

Run: `git commit --allow-empty -m "engine: phase 1 hygiene check"` (then immediately delete or amend if all is well). Verify the pre-commit hook passes within budget.

**Commit:** No commit (this task is a verification gate).
<!-- END_TASK_7 -->

---

## Phase Done When

- All 6 implementation tasks (Tasks 1–6) committed.
- Each test added is either passing (locking in current correct behavior, e.g., AC1.2 wildcard reducer) or `#[ignore]`-marked with a comment pointing to the phase that activates it.
- `cargo test -p simlin-engine` passes within the 180s budget.
- Running `cargo test -p simlin-engine -- --ignored` shows the expected red tests, each with a clear failure message that documents the desired post-refactor behavior.
- No production code (outside test scaffolding like the `RefShape` enum stub) is modified.
- Pre-commit hook passes.
