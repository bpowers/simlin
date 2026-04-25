# Phase 4 — Loop Classification Updates and Shape Annotation

**Goal:** Annotate `Link.shape` at loop construction time so loop-score generation (Phase 3) resolves the right link-score names. Audit `build_element_level_loops` against the new sparser edge set, simplify dead branches, run the full LTM integration suite, and tighten any existing assertions whose looseness was a workaround for the now-fixed bugs.

**Architecture:** `build_element_level_loops` (`db_ltm.rs:1821-2078`) and `CausalGraph::circuit_to_links` (`ltm.rs:1410-1424`) construct `Link`s without shape today. Phase 4 adds a post-construction shape-annotation pass that parses the `[...]` subscript out of element-level node names. The cross-element branch's "diagonal-link-score approximation" comment becomes accurate (the spurious all-pairs circuits that motivated it are gone after Phase 2).

**Tech Stack:** Rust, `Link` struct in `ltm.rs`, `Loop` construction in `db_ltm.rs`, integration tests in `tests/simulate_ltm.rs`.

**Codebase verified:** 2026-04-25 (Phase 4 codebase-investigator confirmed: `Link` constructor sites at `db_ltm.rs:2047`, `ltm.rs:1416` (`circuit_to_links`), `ltm.rs:1433` (`path_to_links`), `ltm.rs:1174` (`find_loops_with_limit`); polarity calc independent of shape; loop dedup uses sorted node-set without shape; layout doesn't consume `Link`; tech-debt #34 already RESOLVED — its relaxed assertions can be tightened after this phase if reasonable).

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### ltm-per-ref-elem-graph.AC4: Loop detection consumes the new edge set
- **ltm-per-ref-elem-graph.AC4.1 A2A loops still detected:** Pure-A2A loops continue to be detected and classified as A2A loops with shared IDs.
- **ltm-per-ref-elem-graph.AC4.2 Legitimate cross-element loops detected:** The cross-element fixture's intended cross-element loops (going through `migration_pressure[*]` and `migration_in[*]` with literal index references) are detected and classified as scalar mixed loops. Spurious cross-element loops induced by today's all-pairs expansion disappear.
- **ltm-per-ref-elem-graph.AC4.3 Existing simulate_ltm tests pass:** All tests in `src/simlin-engine/tests/simulate_ltm.rs` pass without modification (or with explicit per-test documentation of golden-data deltas).

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Add `shape: Option<RefShape>` to `Link` and update existing constructors

**Verifies:** infrastructure for AC4.1, AC4.2

**Files:**
- Modify: `src/simlin-engine/src/ltm.rs:66-72` (`Link` struct)
- Modify: `src/simlin-engine/src/ltm.rs:1416-1420` (`circuit_to_links`)
- Modify: `src/simlin-engine/src/ltm.rs:1430-1437` (`path_to_links`)
- Modify: `src/simlin-engine/src/ltm.rs:1170-1175` (`find_loops_with_limit` direct push)
- Modify: `src/simlin-engine/src/db_ltm.rs:2047-2051` (mixed/scalar branch)

**Implementation:**
This task may have already been done in Phase 3 Task 6 (the same edit was specified there). If Phase 3 added `shape: Option<RefShape>` to `Link`, skip the struct edit and proceed to constructor updates. Otherwise:

1. Add `pub shape: Option<RefShape>` to `Link` (after `polarity`).
2. Update all `Link` constructors in `ltm.rs` and `db_ltm.rs` to include `shape: None`.
3. Verify `Link` is `Clone`, `Debug`, `PartialEq`, `Eq`, and `Hash` (or equivalent derives) — `RefShape` is already those after Phase 2.

**Testing:**
None directly. Verify the workspace compiles:

**Verification:**
Run: `cargo build -p simlin-engine`. Expected: clean build.
Run: `cargo test -p simlin-engine`. Expected: existing tests pass; `Link.shape` is always `None`, so behavior is unchanged.

**Commit:** `engine: add Link.shape field, default None at all constructor sites`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Shape annotation helper — `infer_link_shape_from_node_name`

**Verifies:** infrastructure for AC4.1, AC4.2

**Files:**
- Modify: `src/simlin-engine/src/db_ltm.rs` (or co-locate with `RefShape` definition)

**Implementation:**
The element-level node names produced by `model_element_causal_edges` carry their subscripts in the string itself (e.g., `pop[nyc]`, `migration_pressure[nyc]`, `share[boston,adult]`). Build a helper that derives `RefShape` from such a name:

```rust
/// Infer the access shape from an element-level node name.
///
/// - `"pop"` -> `Bare`
/// - `"pop[nyc]"` -> `FixedIndex(vec!["nyc"])`
/// - `"share[boston,adult]"` -> `FixedIndex(vec!["boston", "adult"])`
/// - `"share[*]"` (unusual; element graph shouldn't contain wildcards) -> `Wildcard`
///
/// Element names are returned canonical (lowercase). The function does not
/// distinguish A2A same-element references from FixedIndex — the element graph
/// uses identical node-name format for both. Callers needing that distinction
/// must consult the loop's circuit structure (e.g., A2A loops have all nodes
/// at the same element across the cycle).
pub(crate) fn infer_link_shape_from_node_name(name: &str) -> RefShape {
    match name.find('[') {
        None => RefShape::Bare,
        Some(start) => {
            let close = match name[start..].find(']') {
                Some(i) => start + i,
                None => return RefShape::Bare,  // malformed; defensive
            };
            let inside = &name[start + 1..close];
            if inside == "*" || inside.contains('*') {
                return RefShape::Wildcard;
            }
            let elems: Vec<String> = inside.split(',').map(str::to_string).collect();
            RefShape::FixedIndex(elems)
        }
    }
}
```

**Testing:**
Add a few unit tests in the same file's `#[cfg(test)] mod tests`:
```rust
#[test]
fn infer_shape_bare() {
    assert_eq!(infer_link_shape_from_node_name("pop"), RefShape::Bare);
}
#[test]
fn infer_shape_fixed_single_dim() {
    assert_eq!(
        infer_link_shape_from_node_name("pop[nyc]"),
        RefShape::FixedIndex(vec!["nyc".to_string()])
    );
}
#[test]
fn infer_shape_fixed_multi_dim() {
    assert_eq!(
        infer_link_shape_from_node_name("share[boston,adult]"),
        RefShape::FixedIndex(vec!["boston".to_string(), "adult".to_string()])
    );
}
```

**Verification:**
Run: `cargo test -p simlin-engine --lib infer_link_shape`. Expected: all pass.

**Commit:** `engine: infer_link_shape_from_node_name helper for loop-link annotation`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Annotate `Link.shape` in `build_element_level_loops`

**Verifies:** AC4.1, AC4.2 (loop-score equations resolve correct link names)

**Files:**
- Modify: `src/simlin-engine/src/db_ltm.rs:1821-2078`

**Implementation:**
For each branch of `build_element_level_loops`, post-process the constructed links to populate `shape`:

**A2A branch** (lines 1932-1975): the loop's links use variable-level (stripped) names. Each link represents same-element access. Set `shape: Some(RefShape::Bare)` on every link in the loop.

**Cross-element branch** (lines 1976-2017): the loop's links use deduplicated variable-level names. The actual element-level access pattern is approximated as a scalar mixed loop. Set `shape: Some(RefShape::Bare)` on every link — the loop-score equation will reference the canonical bare-name link scores.

> **Note:** Phase 5's measurement task should specifically watch for whether this approximation produces sensible cross-element loop-score values on the cross_element_ltm fixture. If the values are clearly nonsensical, a future iteration could promote per-link FixedIndex annotation here. For now, this matches today's behavior post-Phase-2.

**Mixed/scalar branch** (lines 2018-2072): the loop's links use element-level names. Per-link shape is determined by parsing the **target's** node name (since the link is *into* the target). Use `infer_link_shape_from_node_name(link.to.as_str())`. If the target carries a subscript (`migration_in[nyc]`), the link is an element-level edge; if the **source** also carries a subscript (`migration_pressure[boston] -> migration_in[nyc]`), the source's subscript indicates whether this is a FixedIndex broadcast (Boston is the literal source element) or a SameElement diagonal (both [nyc]). Compare the subscripts:

```rust
let from_subscript = parse_subscript(link.from.as_str());
let to_subscript = parse_subscript(link.to.as_str());
let shape = match (from_subscript, to_subscript) {
    (None, _) => RefShape::Bare,         // bare source
    (Some(fs), Some(ts)) if fs == ts => RefShape::Bare,  // same-element diagonal
    (Some(fs), _) => RefShape::FixedIndex(fs.split(',').map(str::to_string).collect()),
};
```

Add a `parse_subscript` helper (returns the inside of the brackets as `Option<String>`).

After annotation, the per-edge name resolved by `link_score_var_name(link.from, link.to, &shape, has_collision=false)` correctly points at:
- The canonical `$⁚ltm⁚link_score⁚{from}→{to}` for Bare edges (today's standard A2A link score).
- The `$⁚ltm⁚link_score⁚{from}[{elem}]→{to}` for FixedIndex broadcast edges (new per-shape variant from Phase 3).

**Testing:**
Add an integration test in `db_ltm_unified_tests.rs` (or a new `db_ltm_shape_annotation_tests.rs`):

```rust
#[test]
fn a2a_loop_links_carry_bare_shape() {
    // Pure A2A: pop[r] -> births[r] -> pop[r]
    let project = TestProject::new("a2a")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "pop * 0.1", None);
    let loops = build_loops_for_test(&project);  // helper that runs build_element_level_loops
    assert!(!loops.is_empty());
    for l in &loops {
        for link in &l.links {
            assert_eq!(link.shape, Some(RefShape::Bare));
        }
    }
}

#[test]
fn cross_element_fixed_index_link_carries_fixed_index_shape() {
    // From the cross_element fixture pattern: migration_pressure[NYC]
    // references population[Boston] (FixedIndex), so the loop link
    // pop -> migration_pressure should carry FixedIndex(Boston) for
    // the broadcast edge.
    let project = TestProject::new("crossel")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .array_aux("mp[Region]", "(pop[NYC] - pop[Boston]) * 0.01");
    // ... set up a loop that includes a FixedIndex edge ...
    // Verify at least one link in some loop has shape FixedIndex.
    // If no loop is detected (this fixture is non-feedback), assert that
    // the absence of such a loop is acceptable; otherwise verify shape.
}
```

The cross-element fixture test case may need a feedback-closed model (e.g., loop back through births). Adapt the fixture to ensure a loop exists; the goal is to exercise FixedIndex shape annotation in a real loop.

**Verification:**
Run: `cargo test -p simlin-engine`. Expected: all tests pass; new shape-annotation tests pass.

**Commit:** `engine: annotate Link.shape in build_element_level_loops`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->

<!-- START_TASK_4 -->
### Task 4: Audit and simplify cross-element branch

**Verifies:** AC4.2 (legitimate cross-element loops detected; spurious ones absent)

**Files:**
- Modify: `src/simlin-engine/src/db_ltm.rs:1976-2017` (cross-element branch)

**Implementation:**
The cross-element branch comments at lines 1976-1992 explain the "approximation" rationale: the spurious cross-element edges from today's NxN expansion required a fallback to scalar approximation. After Phase 2, those spurious edges are gone. Two outcomes are possible:

(a) **Cross-element circuits still exist for legitimate reasons** (e.g., wildcard reducers in feedback loops, or genuine cross-element fixed-index broadcasts that happen to close a cycle). The branch's logic is still correct but the comment should be updated to reflect that the remaining cases are legitimate, not workarounds.

(b) **The branch becomes effectively unreachable** for FixedIndex-only patterns. The wildcard-reducer case (e.g., `total_pop = SUM(pop[*])` in a model where `total_pop` feeds back into `pop[*]`) still produces cross-element circuits.

Run the existing test suite to determine which outcome dominates. If (a), update the comment block (lines 1976-1992) to remove the "workaround" framing. If (b), keep the branch as a guard for the wildcard-reducer case; remove only the workaround framing.

Suggested updated comment (replacing lines 1976-1992):
```rust
} else if is_cross_element {
    // Cross-element circuits: a circuit where nodes have different element
    // subscripts on the same dimension (e.g., pop[nyc] -> total_pop ->
    // pop[boston] -> total_pop -> pop[nyc] via a wildcard reducer).
    //
    // After the Phase 2 per-reference element graph (FixedIndex no longer
    // expands to NxN), most cross-element circuits originate from
    // wildcard reducers like SUM(x[*]). For these we extract the unique
    // variable-level cycle and emit a scalar Loop using diagonal
    // link-score variable names. The actual off-diagonal sensitivities
    // are not directly available, but the wildcard reducer link score
    // already encodes the cross-element contribution as a single A2A
    // value, so the diagonal approximation captures the loop structure.
```

**Testing:**
Existing tests cover the wildcard-reducer cross-element case (`cross_element_loop_through_sum_reducer` in `db_element_graph_tests.rs:438`). After this task, verify those still pass.

**Verification:**
Run: `cargo test -p simlin-engine`. Expected: all tests pass.

**Commit:** `engine: simplify cross-element branch comments after spurious-edge removal`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Run full simulate_ltm suite and document loop-count deltas

**Verifies:** AC4.3

**Files:**
- Modify (potentially): `src/simlin-engine/tests/simulate_ltm.rs` (test assertions)
- Possibly: golden-data files in `test/cross_element_ltm/`, `test/arrayed_population_ltm/`, `test/hero_culture_ltm/` if any exist

**Implementation:**
Run the full `simulate_ltm` integration suite and produce a written report:

```bash
cargo test -p simlin-engine --test simulate_ltm 2>&1 | tee /tmp/phase4-simulate-ltm.log
```

For every failing or unexpectedly-passing test, investigate:
1. **Loop count changed?** Verify the new count is correct against the design plan's truth table for the fixture's equations. If correct, update the test assertion AND add a comment linking to this implementation phase.
2. **Loop ID changed?** Loop IDs (`r1`, `b2`, `u3`) are deterministic by content key. After Phase 2, the content-key set may shift. If a test asserts `id == "r1"`, it may need updating to assert presence of the appropriate loop by structure, not by ID.
3. **Per-element score changed?** Tech-debt #34's relaxed assertions ("at least one slot non-zero") were workarounds for the previously-broken broadcast bug. Now slots that were previously masked may legitimately be zero or non-zero. Tighten where reasonable; maintain the looser form where fixture dynamics make per-slot equality fragile.

For `test_arrayed_population_ltm_exhaustive`: the test was relaxed because the LA element is at equilibrium. Verify that's still true post-Phase-2 (no new spurious edges into LA's loop). If so, keep the relaxed form; if false, document.

For `test_cross_element_ltm_exhaustive`: the assertion at lines 4222-4225 ("at least one loop has at least one slot non-zero") should remain valid. If the spurious-cross-element-loops disappearance reduces total loop count below today's value, the test still passes. If a loop that today has non-zero score becomes uniformly zero after the refactor, investigate carefully — this could indicate a Phase 3 edge-emission bug.

For tests that hard-assert exact circuit counts (`a2a_produces_n_element_identical_loops`, `cross_element_loop_through_sum_reducer`, `discovery_arms_race_3party`): these should NOT break per the codebase investigation, but verify.

For each modified test, the commit message should list the test name and the before/after value.

**Testing:**
N/A — this task IS testing.

**Verification:**
Run: `cargo test -p simlin-engine`. Expected: all tests pass.

**Commit:** `engine: re-validate simulate_ltm against per-reference loop structure` (with concrete deltas in the body)
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Tighten relaxed assertions where post-fix dynamics permit

**Verifies:** AC4.3 (assertions reflect actual post-fix behavior)

**Files:**
- Modify (potentially): `src/simlin-engine/tests/simulate_ltm.rs`

**Implementation:**
Tech-debt #34's resolution note explicitly mentions:
> Two pre-existing tests (`test_arrayed_population_ltm_exhaustive`, `test_cross_element_ltm_exhaustive`) had assertions that passed pre-fix only because the broadcast bug hid equilibrium elements; relaxed to "at least one slot non-zero" to match real fixture semantics.

After Phase 4, examine these tests:

1. **`test_arrayed_population_ltm_exhaustive`**: the LA element being at equilibrium is a fixture property (death rate = birth rate exactly), not a refactor artifact. The relaxed form should remain. If the test's intention was originally to verify ALL slots non-zero, that would be wrong for this fixture regardless of bug status.

2. **`test_cross_element_ltm_exhaustive`**: examine each loop's per-slot dynamics. If specific slots can now be tightened to "non-zero" after Phase 2/3, do so with a comment. Otherwise leave the relaxed form.

Investigation criteria for tightening: a loop's slot can be tightened to "non-zero" only if the fixture guarantees the slot's source element has non-equilibrium dynamics. For the cross_element fixture, the migration_pressure-driven feedback creates non-equilibrium dynamics in both NYC and Boston (the asymmetric initial conditions and pressure-based migration ensure both populations change). Both element slots should now be non-zero.

If tightening is reasonable for any test, add a comment:
```rust
// Tightened in 2026-04-25-ltm-per-ref-elem-graph: previously the broadcast
// bug + spurious-edges combination made some slots appear non-zero only
// because of bug interactions. Post-refactor, both [nyc] and [boston] have
// genuine non-zero loop scores driven by migration_pressure feedback.
assert!(loop_scores_per_slot.iter().all(|s| !s.is_empty() && s[some_t] != 0.0));
```

**Testing:**
Add tightened assertions where justified. Re-run the suite to confirm.

**Verification:**
Run: `cargo test -p simlin-engine --test simulate_ltm`. Expected: all pass with tightened assertions.

**Commit:** `engine: tighten cross_element loop-score assertions post per-reference fix`
<!-- END_TASK_6 -->

<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_7 -->
### Task 7: Phase 4 wrap-up — pre-commit hook end-to-end

**Verifies:** AC5.2 (interim — pre-commit budget honored)

**Files:**
- None modified

**Implementation:**
Trigger the pre-commit hook by amending HEAD. Confirm pass within budget.

**Verification:**
Run: `git commit --amend --no-edit`. Expected: pre-commit prints "All pre-commit checks passed!".

**Commit:** No new commit.
<!-- END_TASK_7 -->

---

## Phase Done When

- All 6 implementation tasks (Tasks 1–6) committed; Task 7 verification gate passes.
- `Link.shape` is populated correctly for all loops produced by `build_element_level_loops`.
- `cargo test -p simlin-engine` passes; all `simulate_ltm` integration tests pass.
- Any test whose assertion was tightened, loosened, or value-shifted has a comment referencing this phase.
- The cross-element branch comments accurately describe post-refactor semantics.
- Pre-commit hook passes within 180s.
