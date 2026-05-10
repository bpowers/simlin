# Human Test Plan: LTM Cross-Element Aggregate Scoring (GH #503)

This plan verifies the implementation in `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md` (6 phases, 28+ commits). It is a backend simulation-engine change with no UI; the procedure below is a manual verification a maintainer can run to confirm the synthetic LTM variables and loop scores carry the per-element attribution the design intends.

Automated coverage: all 22 acceptance criteria (`ltm-503-cross-element-agg.AC1.1` .. `AC7.2`) have passing tests — see the traceability table at the end and `docs/implementation-plans/2026-05-09-ltm-503-cross-element-agg/test-requirements.md`.

## Prerequisites

- `./scripts/dev-init.sh` (idempotent environment setup).
- From the repo root:
  - `cargo test -p simlin-engine` passing.
  - `cargo test -p simlin-engine --features file_io --test simulate_ltm` passing.
  - `cargo clippy -p simlin-engine --all-targets -- -D warnings` clean.
  - `cargo build -p simlin-cli` (the `simlin` CLI; `cargo run -p simlin-cli -- ...` works too).
- Reference reading: `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md` (the design) and `docs/design/ltm--loops-that-matter.md` "Aggregate Nodes" section (the implemented behavior).
- A scratch directory for the hand-built model XMILE files (e.g. `/tmp/ltm-verify/`).

Note on naming: synthetic LTM variable columns use the U+205A "two dot punctuation" separator (`⁚`) and the U+2192 arrow (`→`), e.g. `$⁚ltm⁚link_score⁚pop[nyc]→$⁚ltm⁚agg⁚0`.

## Phase 1: Inlined-reducer aggregate node (the design's `share[r] = pop[r] / SUM(pop[*])` case)

Build a tiny XMILE model (`/tmp/ltm-verify/share.stmx`): 2-region `Region = {NYC, Boston}`; `pop[Region]` stock with heterogeneous inits `pop[NYC] = 1000`, `pop[Boston] = 10`, single inflow `update`; `share[Region]` aux = `pop / SUM(pop[*])` (apply-to-all); `update[Region]` flow = `share * pop * 0.01` (the `* pop` keeps the feedback curved so link scores stay non-degenerate); Euler, `0..5`, `dt 1`.

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo run -p simlin-cli -- simulate --xmile /tmp/ltm-verify/share.stmx --ltm > /tmp/ltm-verify/share.tsv` | Exits 0; TSV header includes synthetic columns. |
| 2 | In the TSV header, look for `$⁚ltm⁚agg⁚0`. | A column `$⁚ltm⁚agg⁚0` is present -- the hoisted `SUM(pop[*])` aggregate node. |
| 3 | Check that the `$⁚ltm⁚agg⁚0` column equals `pop[nyc] + pop[boston]` at every row. | They match to floating-point tolerance -- the agg aux value is the same expression the model evaluates inline. |
| 4 | Look for `$⁚ltm⁚link_score⁚pop[nyc]→$⁚ltm⁚agg⁚0` and `$⁚ltm⁚link_score⁚pop[boston]→$⁚ltm⁚agg⁚0` columns. | Both present (one scalar link score per source element of the reducer). |
| 5 | For a row `t` where `Δ$⁚ltm⁚agg⁚0 = agg(t) - agg(t-1) ≠ 0`: compute `\|Δpop[nyc] / Δagg\|` and `\|Δpop[boston] / Δagg\|` from the `pop[nyc]`/`pop[boston]` columns and compare to the two link-score values at row `t`. | They match within ~`1e-6`. **Crucially the two values differ markedly** -- `pop[nyc]` (the 1000-region) dominates `Δagg`, so its factor is near 1 and `pop[boston]`'s is near 0. A single lumped "Δ-aggregate(share)/Δshare" link score would give the same value for both elements; this confirms the per-element `\|Δpop[d]/Δagg\|` factor of the chain rule is present and non-constant across `d` (the issue's motivating case, AC4.5). |
| 6 | Look for `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚0→share[nyc]` and `...→share[boston]` columns. | Both present (one scalar link score per target element of the agg's consumer). |
| 7 | Look for `$⁚ltm⁚link_score⁚pop→share` (the Bare numerator's link score). | Present -- the `pop[r]` numerator reference is scored separately from the `SUM(pop[*])` reference (the diagonal conflation is resolved by construction). |
| 8 | Confirm **no** column name contains `⁚wildcard` or `⁚dynamic`. | None -- the per-shape Wildcard/Dynamic link-score path is retired (AC5.1). |
| 9 | Inspect the `$⁚ltm⁚loop_score⁚...` columns. For the cross-element-through-agg loop, pick the loop-score column that is the product of `pop[nyc]→agg`, `agg→share[boston]`, `share→update`[boston slot], `update→pop`[boston slot] -- verify the column equals that product at every row `t >= 2`. | The loop-score series == product of the un-trimmed per-element link scores; the cross-element coupling through the aggregate (`pop[d]→agg→share[e]`, `d≠e`) is accounted for, not collapsed to a diagonal A2A score (AC4.2). |
| 10 | (Structure check, optional) Inspect via the engine API: confirm `model_detected_loops` for this model surfaces *no* loop whose variable list contains `$⁚ltm⁚agg⁚0`. | The synthetic agg node is trimmed from every *reported* loop (like the internal stocks of `DELAY3`/`SMOOTH`) -- it exists only internally to build the loop-score equations. |

## Phase 2: Cross-element loops scored element-level (the `cross_element_ltm` fixture)

The repo fixture `test/cross_element_ltm/cross_element.stmx` already has the shape: per-element `migration_pressure[r] = (population[r] - population[other]) * 0.01`, `migration_in[NYC] = MAX(migration_pressure[Boston] * -1, 0)` (and symmetric), and a scalar `total_population = SUM(population[*])`.

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo run -p simlin-cli -- simulate --xmile test/cross_element_ltm/cross_element.stmx --ltm > /tmp/ltm-verify/cross.tsv` | Exits 0. |
| 2 | In the header, find `$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in` (an A2A score dimensioned over Region; columns are `[nyc]` then `[boston]`). | Present. Its NYC slot has `\|value\| ≈ 1` at every step `t >= 2` (because `migration_in[NYC] = MAX(-migration_pressure[Boston], 0)` and `migration_pressure[Boston] < 0` throughout), and its Boston slot is identically `0`. (Pre-fix this slot carried a `"0"`-partial-derived value far from 1 -- AC1.4.) |
| 3 | Find the `$⁚ltm⁚loop_score⁚...` column for the loop `population[nyc] → migration_pressure[boston] → migration_in[nyc] → population[nyc]`. Read the three per-element link scores it references (`...population[nyc]→migration_pressure"[boston]`, `...migration_pressure[boston]→migration_in"[nyc]`, `...migration_in→population"[nyc]`) and verify their product equals the loop-score column at every `t >= 2`. | Match within `1e-6`, non-zero at some step. The cross-element loop is scored on the element-level path with element-subscripted link-score refs -- *not* the unsubscripted A2A diagonal scores (e.g. it does not reference `migration_pressure→migration_out`) -- AC2.1/AC2.2. |
| 4 | Confirm the symmetric loop `population[boston] → migration_pressure[nyc] → migration_in[boston] → population[boston]` also has a `loop_score` column referencing the analogous subscripted scores (its value may be identically 0 -- `migration_in[Boston] = MAX(-migration_pressure[NYC], 0)` with `migration_pressure[NYC] > 0` -- documented fixture behavior). | The loop is enumerated with the right subscripted refs (AC2.3). |
| 5 | (Discovery mode) Inspect via the engine API: run `discover_loops_element_level` on the fixture; confirm some discovered loop has a link `population[nyc] → migration_pressure[boston]` (or the symmetric `population[boston] → migration_pressure[nyc]`). | Discovery (strongest-path) finds the genuine cross-element edge, not merely "some subscripted loop" (AC3.1). |

## Phase 3: Partial reduce over one axis (`agg[D1] = SUM(matrix[D1,*])`)

Build `/tmp/ltm-verify/partial.stmx`: `D1 = {a,b}`, `D2 = {x,y}`; `matrix[D1,D2]` stock with distinct inits (`a,x=100`, `a,y=150`, `b,x=200`, `b,y=250`), inflow `growth`; `row_sum[D1]` aux = `SUM(matrix[D1,*])`; `total` aux = `SUM(row_sum[*])`; `growth[D1,D2]` flow = `matrix * total * 0.000001`; Euler, `0..10`, `dt 1`.

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo run -p simlin-cli -- simulate --xmile /tmp/ltm-verify/partial.stmx --ltm > /tmp/ltm-verify/partial.tsv` | Exits 0. |
| 2 | Find columns `$⁚ltm⁚link_score⁚matrix[a,x]→row_sum[a]`, `$⁚ltm⁚link_score⁚matrix[a,y]→row_sum[a]`, and the two `[b,...]→row_sum[b]` counterparts. | All four present -- the source subscript carries both axes, the target subscript only the surviving axis. (`row_sum[D1] = SUM(matrix[D1,*])` is a whole-RHS reducer, so `row_sum` is a variable-backed agg, not a synthetic `$⁚ltm⁚agg⁚n` -- AC4.3.) |
| 3 | For row `a` at a step where the row changed: `\|matrix[a,x]→row_sum[a]\| + \|matrix[a,y]→row_sum[a]\|` should be ≈ 1 (a SUM partial reduce splits the row delta; both inflows are positive so the deltas share a sign). Confirm neither is identically 0 across all steps and neither is always exactly magnitude 1. | Sum-of-magnitudes ≈ 1 at every changed step; values are non-degenerate fractions strictly between 0 and 1 (AC4.6). |
| 4 | Confirm there is **no** Bare-A2A `$⁚ltm⁚link_score⁚matrix→row_sum` column (no element subscript on either side). | Absent -- that would broadcast over D1 in the discovery parser and produce wrong edges. |
| 5 | Find at least one `$⁚ltm⁚loop_score⁚...` column whose equation references one of the `matrix[d1,d2]→row_sum[d1]` link scores. | Present -- the partial-reduce link score contributes to a loop score. |

## Phase 4: No-regression (scalar and pure-A2A models, plus WRLD3)

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo run -p simlin-cli -- simulate --xmile test/logistic_growth_ltm/logistic_growth.stmx --ltm`, compare against `test/logistic_growth_ltm/ltm_results.tsv` (or confirm `cargo test ... simulate_ltm simulates_population_ltm` is green). | The simulated LTM output matches the golden TSV exactly -- scalar models have no reducers and no per-element-equation link-score targets, so no aggregate nodes and no element-graph change. The golden TSV is byte-unchanged by this branch (AC6.1). |
| 2 | Confirm no `$⁚ltm⁚agg⁚...`, `⁚wildcard`, or `⁚dynamic` columns appear in the logistic-growth output. | None -- the scalar/A2A path is untouched. |
| 3 | Run the WRLD3 LTM smoke (`cargo test ... --test wrld3_ltm_panic wrld3_ltm_compilation_finishes_in_time`). | Compiles + simulates with LTM enabled within the time budget; auto-flips to discovery (166-node SCC > the 50-node gate), so it emits no `loop_score` synthetic vars but does not panic or hang. The element graph collapses to the variable graph (WRLD3 is scalar) -- unchanged from pre-Phases-1-6. |
| 4 | Run the full `cargo test --workspace` (the AC6.2 wall-clock gate) and check elapsed time. | Completes green within ~180s; the pre-commit hook (`scripts/pre-commit`) passes end-to-end. |

## Phase 5: Docs and tracking spot-check

| Step | Action | Expected |
|------|--------|----------|
| 1 | `python3 scripts/check-docs.py` | "Documentation link check passed." (exit 0). |
| 2 | Open `docs/design/ltm--loops-that-matter.md`: confirm the Naming table has a `$⁚ltm⁚agg⁚{n}` row and per-source/per-target-element link-score rows; there is an "Aggregate Nodes" subsection describing the hoist + chain-rule scoring + trim; the Array-Support truth table's Wildcard/DynamicIndex rows say "rerouted through `$⁚ltm⁚agg⁚n` ... O(N+M)" (with the conservative-slice / bare-dynamic-index carve-out still O(N*M)); no stale `⁚wildcard` references describing it as an emitted variable. | All present and accurate (AC7.1). |
| 3 | Open `src/simlin-engine/CLAUDE.md`: confirm the freshness date is `2026-05-09`; the `ltm_agg.rs`/`db_analysis.rs`/`db_ltm.rs`/`ltm/types.rs`/`ltm_augment.rs` bullets mention `enumerate_agg_nodes`, the `$⁚ltm⁚agg⁚{n}` family, the retired `⁚wildcard`/`⁚dynamic` suffixes, the scalar→arrayed `{from}→{to}[{elem}]` naming, element-subscripted cross-element loops, and `LtmSyntheticVar.equation: datamodel::Equation`. (`AGENTS.md` is a symlink of `CLAUDE.md`.) | All present. |
| 4 | Open `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`: confirm the "Re-measurement after the cross-element aggregate-scoring work (2026-05-09)" subsection has a table with post-Phases-1-5 element-edge / element-SCC / tiered fast-slow / slow-SCC / auto-flip numbers for `cross_element_ltm`, `arrayed_population_ltm`, `hero_culture_ltm`, WRLD3-03, plus the new `share[r]=pop[r]/SUM(pop[*])`-with-feedback (3 and 8 regions) and `MEAN(pop[*])`-with-feedback fixtures, cross-linked to `2026-05-09-ltm-503-cross-element-agg.md`. Optionally re-run `cargo run --release --example ltm_full_bench -- <fixture>` for one or two listed fixtures to spot-check the numbers. | The subsection exists with the numbers and the cross-link. |
| 5 | Open `docs/tech-debt.md`: confirm items #20, #26, #34 carry a RESOLVED/superseded note with the forward-pointer to `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md` and the implementing commit hashes. | Present. |
| 6 | `gh issue view 503` and `gh issue view 488`. | #503 is CLOSED with a comment referencing the implementing commit range, noting the aggregate-node approach and the follow-ups (#514/#515/#516); no `fixes #503` keyword in the commits (manual close). #488's array bullet for #503 reads `- [x] #503 -- ...` with the commit refs and the design-plan pointer (AC7.2). |

## End-to-end: chain rule through an aggregate node

This is the single most important behavioral assertion of the whole change -- that an inlined reducer in a cross-element feedback loop is scored by the chain rule (`pop[d] → agg → target`), recovering each source element's *fractional contribution to the aggregate's velocity*, rather than the old "diagonal A2A link score" approximation that systematically over-counts each element's individual contribution when element magnitudes are heterogeneous.

Using the Phase 1 model (`pop[NYC]=1000`, `pop[Boston]=10`, `share[r] = pop[r]/SUM(pop[*])`, `update[r] = share[r]*pop[r]*0.01`), after `simulate --ltm`:

1. Read, from the TSV at a step `t >= 2` where `Δagg ≠ 0`: `pop[nyc]→agg`, `pop[boston]→agg`, `agg→share[nyc]`, `share→update` (NYC slot), `update→pop` (NYC slot).
2. Multiply the four factors on the NYC self-loop path: `pop[nyc]→agg * agg→share[nyc] * share→update[nyc] * update→pop[nyc]`.
3. Find the reported loop `pop[nyc]→share[nyc]→update[nyc]→pop[nyc]` (the agg node is trimmed out of the displayed links) and read its `$⁚ltm⁚loop_score⁚...` column.
4. **Expected:** the loop-score column == the four-factor product at every such step (within `1e-6`). Repeat for the Boston self-loop and for the cross-element loop (`pop[nyc]→agg→share[boston]→update[boston]→...→pop[boston]→agg→share[nyc]→update[nyc]→...→pop[nyc]`); the cross-element loop's score is the product of *all* the un-trimmed per-element link scores along its (agg-crossing-twice) path. Also confirm: if you build the *same* model without the `SUM(pop[*])` reducer (e.g. `share[r] = pop[r] / 1010`, a constant), the loop score for the bare-numerator loop is a 3-factor product (`pop[r]→share[r] * share[r]→update[r] * update[r]→pop[r]`) -- the contrast tells the aggregate path apart from the numerator path.

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| `ltm-503-cross-element-agg.AC1.1` | `ltm_augment.rs::test_arrayed_link_score_population_to_migration_pressure_fixed_nyc` | Phase 2 step 2 |
| `ltm-503-cross-element-agg.AC1.2` | `ltm_augment.rs::test_arrayed_link_score_population_to_migration_pressure_fixed_boston` | Phase 2 step 2 |
| `ltm-503-cross-element-agg.AC1.3` | `db_ltm_tests.rs::test_stock_to_flow_link_score_handles_arrayed`, `ltm_augment.rs::test_arrayed_link_score_stock_to_flow_per_element_partials` | Phase 1 step 4 |
| `ltm-503-cross-element-agg.AC1.4` | `simulate_ltm.rs::test_cross_element_link_score_migration_in_arrayed_partials` | Phase 2 step 2 |
| `ltm-503-cross-element-agg.AC1` (refactor) | (all pre-existing tests stay green) | Phase 4 step 4 |
| `ltm-503-cross-element-agg.AC2.1` | `simulate_ltm.rs::test_cross_element_ltm_loop_score_uses_element_path`, `..._exhaustive` | Phase 2 step 3 |
| `ltm-503-cross-element-agg.AC2.2` | `simulate_ltm.rs::test_cross_element_ltm_loop_score_value_matches_hand_calc` | Phase 2 step 3 |
| `ltm-503-cross-element-agg.AC2.3` | `simulate_ltm.rs::test_cross_element_ltm_symmetric_loop_enumerated` | Phase 2 step 4 |
| `ltm-503-cross-element-agg.AC2.4` | `simulate_ltm.rs::test_a2a_pure_dimension_loop_scores`, `db_ltm_unified_tests.rs::a2a_loop_links_use_variable_level_names` | Phase 4 steps 1-2 |
| `ltm-503-cross-element-agg.AC2.5` | `db_ltm_unified_tests.rs::scalar_model_loop_score_has_no_element_subscript` | Phase 4 steps 1-2 |
| `ltm-503-cross-element-agg.AC3.1` | `simulate_ltm.rs::test_cross_element_ltm_discovery` | Phase 2 step 5 |
| `ltm-503-cross-element-agg.AC3.2` | `simulate_ltm.rs::test_scalar_reducer_loop_discovery`, `db_ltm_unified_tests.rs::scalar_reducer_loop_score_uses_per_element_link_scores`, `simulate_ltm.rs::test_scalar_reducer_loop_score_value_matches_hand_calc` | (build a `total_pop = SUM(population[*]); migration[r] = total_pop*c - population[r]*c` model to verify discovery directly) |
| `ltm-503-cross-element-agg.AC3.3` | `ltm_finding.rs::test_parse_link_offsets_scalar_to_arrayed` | (parser-level; exercised transitively by Phase 2 step 5) |
| `ltm-503-cross-element-agg.AC4.1` | `ltm_agg.rs::subexpression_reducer_mints_one_synthetic_agg`, `db_element_graph_tests.rs::element_graph_wildcard_reducer_plus_bare_truthful`, `db_ltm_unified_tests.rs::per_shape_link_scores_for_share_with_sum`, `agg_aux_emitted_for_hoisted_reducer` | Phase 1 steps 2, 4, 6, 7 |
| `ltm-503-cross-element-agg.AC4.2` | `db_ltm_unified_tests.rs::cross_element_loop_through_agg_is_recovered`, `simulate_ltm.rs::test_no_duplicate_ltm_vars_with_agg_routed_and_direct_edge`, `test_agg_aux_value_matches_reducer` | Phase 1 steps 3, 9, 10 + End-to-end |
| `ltm-503-cross-element-agg.AC4.3` | `ltm_agg.rs::whole_rhs_scalar_reducer_is_its_own_agg` (+ siblings), `db_element_graph_tests.rs::element_graph_whole_rhs_scalar_reducer_is_its_own_agg_node`, `db_ltm_unified_tests.rs::no_agg_aux_for_whole_rhs_reducer` | Phase 3 step 2 |
| `ltm-503-cross-element-agg.AC4.4` | `ltm_agg.rs::nested_reducers_mint_two_aggs`, `ast_identical_reducers_dedupe`, `enumeration_is_deterministic_under_variable_reordering` | (build a `x = SUM(a[*]) / SUM(b[*])` model and confirm two `$⁚ltm⁚agg⁚n` columns) |
| `ltm-503-cross-element-agg.AC4.5` | `simulate_ltm.rs::test_agg_link_scores_heterogeneous_match_hand_calc` | Phase 1 step 5 + End-to-end |
| `ltm-503-cross-element-agg.AC4.6` | `ltm_augment.rs::test_generate_reduced_{sum,mean,min,max,constant,nested}_equation`, `test_generate_full_reduce_unchanged_after_refactor`, `db_ltm_unified_tests.rs::partial_reduce_emits_per_source_element_scalar_link_scores`, `ltm_finding.rs::test_parse_link_offsets_partial_reduce_passthrough`, `simulate_ltm.rs::test_partial_reduce_cross_element_loop`, `test_full_reduce_still_works_after_partial_reduce_support` | Phase 3 (all steps) |
| `ltm-503-cross-element-agg.AC4.7` | `simulate_ltm.rs::test_discovery_loop_through_agg_scored_on_untrimmed_path` | Phase 1 steps 9-10 (discovery side) |
| `ltm-503-cross-element-agg.AC5.1` | `db_ltm_unified_tests.rs::no_wildcard_or_dynamic_link_scores_for_reducer_models`, `per_shape_link_scores_for_share_with_sum`, `ltm_augment.rs::link_score_name_wildcard_dynamic_collapse_to_bare`; clippy `--all-targets -D warnings` | Phase 1 step 8; Phase 4 step 2 |
| `ltm-503-cross-element-agg.AC5.2` | (code-shape; clippy confirms no dead branch) | Phase 5 -- read `ltm_augment.rs::shape_aware_source_ref` |
| `ltm-503-cross-element-agg.AC6.1` | `simulate_ltm.rs::simulates_population_ltm`, `test_arrayed_population_ltm_exhaustive`/`_discovery`, `wrld3_ltm_panic.rs::wrld3_ltm_compilation_finishes_in_time` | Phase 4 steps 1-3 |
| `ltm-503-cross-element-agg.AC6.2` | operational (`cargo test --workspace`, pre-commit hook) | Phase 4 step 4 |
| `ltm-503-cross-element-agg.AC7.1` | `scripts/check-docs.py`, `simulate_ltm.rs::measurement_postscript_{cross_element_ltm,arrayed_population_ltm}` | Phase 5 steps 1-5 |
| `ltm-503-cross-element-agg.AC7.2` | (GitHub state -- no automatable test) | Phase 5 step 6 |
