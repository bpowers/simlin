# Human Test Plan: LTM Array Support

## Prerequisites

- Rust toolchain installed; `./scripts/dev-init.sh` has been run
- All automated tests pass: `cargo test -p simlin-engine --features file_io`
- Branch: `ltm-arrays`

## Phase 1: Automated Test Suite Execution

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --features file_io -- db_ltm` | All db_ltm_tests, db_ltm_unified_tests, and db_ltm_module_tests pass |
| 2 | Run `cargo test -p simlin-engine --features file_io -- db_element_graph` | All 12 element graph tests pass (7 edge tests + 4 loop/partition tests + 1 scalar identity test) |
| 3 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm` | All simulate_ltm integration tests pass, including the 23 new arrayed-model tests |
| 4 | Run `cargo test -p simlin-engine --features file_io -- ltm_finding` | All ltm_finding unit tests pass (parse_link_offsets, SearchGraph, rank_and_filter, assign_loop_ids) |

## Phase 2: E2E Model Validation

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm test_arrayed_population_ltm_exhaustive -- --nocapture` | Test passes; output shows non-zero link scores for 3 regions, loop scores with 3 slots, relative scores summing to ~1.0 per element |
| 2 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm test_arrayed_population_ltm_discovery -- --nocapture` | Test passes; discovery finds element-subscripted loops that match exhaustive structural analysis |
| 3 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm test_cross_element_ltm_exhaustive -- --nocapture` | Test passes; output shows cross-element edges (population[nyc]->total_population, population[boston]->total_population), cycle partitions grouping connected stocks |
| 4 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm test_cross_element_ltm_discovery -- --nocapture` | Test passes; discovery finds loops with element-subscripted variables in cross-element model |

## Phase 3: Backward Compatibility Regression

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm simulates_population_ltm` | Pre-existing logistic growth LTM test still passes with <5% tolerance against reference TSV |
| 2 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm hero_culture_loop_sign_continuity` | Pre-existing hero culture sign continuity test passes (no sign discontinuities) |
| 3 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm discovery_logistic_growth_finds_both_loops` | Pre-existing discovery test finds exactly 2 loops |
| 4 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm discovery_arms_race_3party` | Pre-existing arms race discovery test finds all 7 loops |
| 5 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm discovery_decoupled_stocks` | Pre-existing decoupled stocks discovery test finds 2 loops |

## Phase 4: Cross-Dimensional Reducer Spot Checks

| Step | Action | Expected |
|------|--------|----------|
| 1 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm test_cross_dim_min_expansion -- --nocapture` | Only NYC (the minimum element at 100) has a significant link score; Boston (200) and LA (300) have ~0 scores |
| 2 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm test_cross_dim_max_expansion -- --nocapture` | Only LA (the maximum element at 300) has a significant link score; NYC and Boston have ~0 scores |
| 3 | Run `cargo test -p simlin-engine --features file_io --test simulate_ltm test_cross_dim_sum_vs_explicit_cross_validation -- --nocapture` | SUM algebraic shortcut produces link scores within 1% of the equivalent explicit scalar model |

## End-to-End: Full Pre-Commit Hook Validation

**Purpose:** Validates that all automated tests pass across the entire test suite (Rust + TypeScript + WASM + Python), confirming no regressions from the LTM array changes.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Stage all changes: `git add -A` | Files staged |
| 2 | Run `git commit -m "test: validate ltm array coverage"` (which triggers pre-commit hook) | Pre-commit hook runs all checks: rustfmt, clippy, cargo test, pnpm lint, pnpm tsc, WASM build, pnpm test, pysimlin tests. All pass |

## Human Verification Required

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| AC8.5: Documentation accuracy | Documentation accuracy is a judgment call about whether prose descriptions match implementation. Automated tests cannot evaluate clarity, completeness, or correctness of natural-language descriptions. | 1. Open `docs/design/ltm--loops-that-matter.md`. Verify it describes: (a) element-level causal graph expansion rules, (b) link score classification (A2A same-dim, scalar-to-arrayed, cross-dimensional), (c) loop score types (A2A shared-ID vs mixed individual-ID), (d) discovery mode on the element-level graph. 2. Open `src/simlin-engine/CLAUDE.md`. Verify it lists: (a) `model_element_causal_edges`, `model_element_loop_circuits`, `model_element_cycle_partitions` as tracked functions in `db_analysis.rs`, (b) updated `LtmSyntheticVar` description mentioning `dimensions` field, (c) `db_element_graph_tests.rs` in the test file listing, (d) `classify_reducer` and `generate_element_to_scalar_equation` in `ltm_augment.rs` description. 3. Spot-check: verify the edge expansion table in the design doc matches the `ElementDependencyKind` enum and expansion logic in `db_analysis.rs`. |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 | `test_a2a_ltm_equation_fragment_compiles`, `test_a2a_ltm_layout_size` | Phase 1, Step 1 |
| AC1.2 | `test_a2a_ltm_previous_per_element` | Phase 1, Step 1 |
| AC1.3 | Existing scalar tests (implicit) | Phase 1, Step 1 |
| AC1.4 | Full regression suite | Phase 3, Steps 1-5 |
| AC2.1-AC2.7 | `db_element_graph_tests.rs` (7 tests) | Phase 1, Step 2 |
| AC3.1-AC3.4 | `db_element_graph_tests.rs` (5 tests) | Phase 1, Step 2 |
| AC4.1-AC4.5 | `simulate_ltm.rs` (5 tests) | Phase 1, Step 3 |
| AC5.1-AC5.8 | `simulate_ltm.rs` (8 tests) | Phase 1, Step 3 + Phase 4 |
| AC6.1-AC6.5 | `simulate_ltm.rs` (3 tests) | Phase 1, Step 3 |
| AC7.1-AC7.5 | `simulate_ltm.rs` (3 tests) + `ltm_finding.rs` (3 tests) | Phase 1, Steps 3-4 |
| AC8.1-AC8.2 | `simulate_ltm.rs` (4 tests) | Phase 2 |
| AC8.3-AC8.4 | Pre-existing test suites | Phase 3 |
| AC8.5 | -- (human only) | Human Verification Required |
