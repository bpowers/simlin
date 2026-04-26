# Human Test Plan: LTM Per-Reference Element Graph

**Implementation plan:** `docs/implementation-plans/2026-04-25-ltm-per-ref-elem-graph/`
**Design plan:** `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`
**Test requirements:** `docs/implementation-plans/2026-04-25-ltm-per-ref-elem-graph/test-requirements.md`

This refactor replaces the variable-level `ElementDependencyKind` classification with a per-AST-reference walker for LTM (Loops That Matter) on arrayed models. Per-shape link scores now have stable names; `parse_link_offsets` understands the new naming. The design plan's measurement postscript records before/after numbers.

This is primarily a backend engine refactor. Most acceptance criteria are covered by automated tests (Rust unit, integration, and proptest). The manual steps below verify documentation freshness, end-to-end fixture behavior, and (where applicable) the diagram editor's LTM panel.

## Prerequisites

- Working directory: `/home/bpowers/src/simlin`
- Run `./scripts/dev-init.sh` once at session start (idempotent).
- `cargo build --release -p simlin-cli` for the fixture inspection commands below.
- `cargo test --workspace` should pass locally on HEAD before starting manual verification.

## Phase 1: Fixture-driven LTM analysis sanity (engine focus)

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | `cargo run --release -p simlin-cli -- simulate test/cross_element_ltm/cross_element.stmx --enable-ltm > /tmp/cross.csv` | Simulation succeeds; CSV header contains link-score columns including the structural A2A names (`population→births`) and the FixedIndex broadcast names (`migration_pressure[nyc]→migration_in`, `migration_pressure[boston]→migration_in`). Confirm there are NO link-score columns for spurious self-edges like `migration_pressure[nyc]→migration_in[nyc]` (the "no-edge" rule from AC1.3). |
| 1.2 | Inspect the CSV: NYC=1000 / Boston=500 initial conditions; one direction of migration is active per timestep. Confirm `migration_in[nyc]` and `migration_in[boston]` evolve non-trivially (not stuck at zero) | Per-shape link scores show up; Boston/NYC asymmetry visible. |
| 1.3 | `cargo run --release -p simlin-cli -- simulate test/arrayed_population_ltm/arrayed_population.stmx --enable-ltm > /tmp/arrayed.csv` | Simulation succeeds; for each loop variable like `$⁚ltm⁚loop_score⁚<id>` confirm 3 element slots (one per region) exist and slots 0/1 (NYC, Boston) carry non-zero values across rows after t≥2; slot 2 (LA, equilibrium fixture) may legitimately be zero. |
| 1.4 | `cargo run --release --example ltm_full_bench -- test/cross_element_ltm/cross_element.stmx 2>&1 \| tee /tmp/cross_bench.txt` | Bench completes successfully; output includes `largest_scc=N` line in the element_edges stage note. Record the value for postscript cross-check. |

## Phase 2: WRLD3 auto-flip threshold

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Verify `test/metasd/WRLD3-03/wrld3-03.mdl` is present | Fixture file exists. |
| 2.2 | `cargo run --release -p simlin-cli -- simulate test/metasd/WRLD3-03/wrld3-03.mdl --enable-ltm 2> /tmp/wrld3.err > /tmp/wrld3.csv` | Simulation succeeds; stderr or output includes the auto-flip diagnostic indicating discovery mode was selected based on element-level SCC size > 50. |
| 2.3 | `cargo run --release --example ltm_full_bench -- test/metasd/WRLD3-03/wrld3-03.mdl` | Bench completes; element-level SCC size remains 166 (variable-level cycles, not element-graph artifacts). Auto-flip threshold (`MAX_LTM_SCC_NODES = 50`) trips as expected. |

## Phase 3: Diagram editor (UI) cross-check (smoke)

This refactor's user-facing impact is via the diagram editor's LTM panel. If the project supports running the app locally:

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | Start the app (`pnpm --filter app dev` or equivalent — see project README) | App starts; LTM panel visible when an arrayed feedback model is open. |
| 3.2 | Open `test/cross_element_ltm/cross_element.stmx` (drag-drop or upload) | Model renders; Loops That Matter panel shows discovered loops. Loop scores reported in the UI for each loop should agree with values written to `/tmp/cross.csv` at matching timesteps. |
| 3.3 | Open `test/arrayed_population_ltm/arrayed_population.stmx` | Model renders; per-region loops appear with element-resolved scores. LA region's loop may show zero (equilibrium fixture); NYC and Boston loops show non-zero scores. |
| 3.4 | Confirm any per-shape link scores (`[nyc]→migration_in` vs `[boston]→migration_in`) are surfaced distinguishably in the panel | Both shape variants visible; no doubly-bracketed (`][`) names; no NaN values. |

## Phase 4: Documentation freshness

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Read `docs/design/ltm--loops-that-matter.md` "Element-Level Causal Graph" section (~lines 525-590) | Opening sentence describes per-AST-reference walking; per-reference table with `RefShape` variants present; multidim partial-fixed conservative rule noted; structural flow→stock short-circuit mentioned; cross-references to tech-debt #20, #25, #26 and the implementation plan present. |
| 4.2 | `grep ElementDependencyKind src/simlin-engine/CLAUDE.md` | No matches. Read line ~44 and confirm the replacement phrase uses the `Bare`/`FixedIndex`/`Wildcard`/`DynamicIndex` shape vocabulary. |
| 4.3 | Read `docs/tech-debt.md` items #20, #26 | Both have `Severity: RESOLVED (2026-04-25)` and a resolution note pointing to commit SHA + design plan, formatted like existing RESOLVED items #23 and #34. |
| 4.4 | Read `docs/tech-debt.md` item #25 | Appended note (dated 2026-04-25) references the Phase 5 measurement postscript and explains the threshold-retention decision. |
| 4.5 | Read `docs/test-plans/2026-04-04-ltm-arrays.md:58` AC8.5 | Spot-check sentence references `RefShape` / `collect_reference_sites` / `emit_edges_for_reference` instead of `ElementDependencyKind`. |
| 4.6 | Read the "Measurement Postscript" section at the end of `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` | Pre/post element-level largest SCC sizes table for `cross_element_ltm` (10/10), `arrayed_population_ltm` (3/3), `hero_culture_ltm` (15/15), and `WRLD3-03` (166/166). Per-fixture commentary present. For each fixture: post-fix SCC ≤ pre-fix SCC. |

## Phase 5: Performance / responsiveness

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | `time bash scripts/pre-commit` | Pre-commit completes well under the 180s `cargo test` wall-clock cap; total wall time on a healthy dev machine ~30-60s. |
| 5.2 | `cargo test -p simlin-engine --lib element_graph` and `cargo test -p simlin-engine --lib db_ltm_unified_tests` | Each unit test completes in <2s on a debug build. |
| 5.3 | `cargo test -p simlin-engine --features file_io --test simulate_ltm` | Full simulate_ltm suite (48 tests) finishes in seconds; no individual test exceeds the per-test budget. |
| 5.4 | Re-run the cross-element CSV from Phase 1.1 twice and diff the files | Determinism preserved — same number of link-score and loop-score columns; identical numerical values across runs. |

## End-to-End: Per-shape link score visibility

**Purpose:** confirm that the per-reference shape distinction observable in unit tests propagates all the way to runtime simulation outputs.

1. Build a small custom test model with `share[Region] = pop / SUM(pop[*])` (use `TestProject` builder or a hand-written `.stmx`).
2. Simulate with `--enable-ltm`; collect link-score columns in the CSV header.
3. Verify exactly two distinct link scores for the (`pop`, `share`) pair: `pop→share` (Bare) and `pop→share⁚wildcard` (Wildcard).
4. Repeat with `rel_pop = pop / pop[NYC]`; verify two distinct link scores: `pop→rel_pop` (Bare) and `pop[nyc]→rel_pop` (FixedIndex).
5. (If UI available) inspect the LTM panel for the same model and confirm both shapes are listed and resolve to non-NaN values.

## End-to-End: Cross-element fixture round-trip

**Purpose:** validate AC1.3's "truthful" edge set semantics against an actual simulator run rather than just the static graph assertion.

1. Load `test/cross_element_ltm/cross_element.stmx` via simlin-cli with `--enable-ltm`.
2. Inspect simulation CSV: confirm columns for migration loops (e.g., `population[nyc] → migration_pressure[boston] → migration_in[nyc] → population[nyc]`) appear and evolve non-trivially.
3. Confirm there are no columns for spurious self-loops `migration_pressure[nyc]→migration_in[nyc]` etc., reflecting the "no-edge" assertions from AC1.3.
4. Spot-check that `total_population` (scalar reduction over `SUM(population[*])`) sums NYC + Boston populations at every timestep.

## Human Verification Required (from test-requirements.md)

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| AC5.1 | Performance/measurement criterion (informational, not a CI gate). Requires running benches and comparing pre/post numbers. | Phase 1 step 1.4, Phase 2 step 2.3, Phase 4 step 4.6 (postscript). |
| AC6.1 | Documentation: prose narrative correctness can only be judged by reading. | Phase 4, step 4.1. |
| AC6.2 | Documentation: single phrase update in CLAUDE.md. | Phase 4, step 4.2. |
| AC6.3 | Documentation: tech-debt items require interpretive judgment. | Phase 4, steps 4.3-4.5. |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 (broadcast) | `element_graph_fixed_index_broadcast_truthful` | Phase 1 step 1.1 |
| AC1.2 (wildcard+bare) | `element_graph_wildcard_reducer_plus_bare_truthful` | E2E: Per-shape link score visibility |
| AC1.3 (cross_element edges) | `test_cross_element_ltm_edge_set_truthful` | Phase 1 step 1.1; E2E: Cross-element fixture round-trip |
| AC1.4 (variable projection) | proptest `element_edges_project_to_variable_edges` | (covered by automated proptest) |
| AC1.5 (multidim partial-fixed) | `element_graph_multidim_partial_fixed_conservative` | (covered by automated test) |
| AC2.1, AC2.2, AC2.3, AC2.4 | `test_partial_equation_*` and `partial_equation_*` | (covered) |
| AC3.1 (per-shape emission) | `per_shape_link_scores_for_share_with_sum`, `fixed_index_link_score_emits_per_element_name` | E2E: Per-shape link score visibility |
| AC3.2 (FixedIndex naming + parser) | `link_score_name_fixed_index`, `test_parse_link_offsets_fixed_index_*` | (covered) |
| AC3.3 (suffixed shapes + parser) | `link_score_name_*`, `test_parse_link_offsets_wildcard_suffix_*` | (covered) |
| AC4.1 (A2A loop links) | `a2a_loop_links_carry_bare_shape` | Phase 1 step 1.3 |
| AC4.2 (mixed scalar + edge aliasing) | `mixed_scalar_loop_score_refs_resolve_to_emitted_names`, `edge_aliasing_bare_and_fixed_index_to_same_source_element` | Phase 3 step 3.4 |
| AC4.3 (full simulate_ltm) | All 48 tests in `tests/simulate_ltm.rs` | Phase 1 steps 1.1, 1.3 |
| AC5.1 (SCC measurement) | (informational, not gated) | Phase 2; Phase 4 step 4.6 |
| AC5.2 (pre-commit hook) | `scripts/pre-commit` 180s cap | Phase 5 step 5.1 |
| AC6.1 (LTM design doc) | (documentation) | Phase 4 step 4.1 |
| AC6.2 (CLAUDE.md) | `grep ElementDependencyKind` returns zero (verified) | Phase 4 step 4.2 |
| AC6.3 (tech-debt + test plan) | (documentation) | Phase 4 steps 4.3-4.5 |
