# Human Test Plan: LTM Arrays Hardening (epic GH #488)

This plan verifies the implementation in `docs/implementation-plans/2026-05-11-ltm-arrays-hardening/` (8 phases, ~30 commits, GH issues #520/#487/#511/#510/#514/#515/#483/#502/#492). It is an engine-internal change -- the LTM (Loops That Matter) static causal-graph analysis and post-simulation loop/link scoring for arrayed variables, plus a thin update to the `libsimlin` FFI consumer. There is **no dedicated UI surface** for arrayed LTM: the app/diagram render detected loops and loop-score displays through the same `simlin_analyze_*` C API the automated FFI tests already round-trip, and `pysimlin` / `@simlin/engine` wrap the identical accessors. The automated suite is the load-bearing verification; the steps below are the few worth a maintainer's time -- mostly **documentation prose review** plus a couple of **end-to-end smoke checks through a real model file**.

Automated coverage: all 38 acceptance criteria (`ltm-arrays-hardening.AC1.1` .. `AC8.4`) have passing tests -- see the traceability table at the end and `docs/implementation-plans/2026-05-11-ltm-arrays-hardening/test-requirements.md`.

## Prerequisites

- `./scripts/dev-init.sh` (idempotent environment setup).
- From the repo root, all green:
  - `cargo test -p simlin-engine`
  - `cargo test -p simlin-engine --features file_io --test simulate_ltm`
  - `cargo test -p simlin --test analysis` (the `libsimlin` FFI tests)
  - `python3 scripts/check-docs.py` (AC8.3 doc-link integrity)
  - `cargo clippy -p simlin-engine --all-targets -- -D warnings` clean
- For the end-to-end model-file checks: `cargo build -p simlin-cli` (`cargo run -p simlin-cli -- ...` works too). Optionally `pnpm build` if exercising `@simlin/serve` or the app.
- Reference reading: `docs/design/ltm--loops-that-matter.md` (the implemented behavior, esp. the reference-site IR / aggregate-node / read-slice sections) and `docs/reference/ltm--loops-that-matter.md` (the modeler-facing notes). The interpretive resolutions for the ambiguous acceptance criteria are recorded in the phase files: `phase_05.md` (cyclic orderings: `(m-1)!/2` distinct orderings for `m >= 3`, mirror reversals skipped, index 0 pinned), `phase_06.md` (STDDEV "constant regime" reframing), `phase_07.md` (`Ast::Arrayed` adopt-first-concrete fold for per-element graphical-function polarity).
- A scratch directory for hand-built model files (e.g. `/tmp/ltm-verify/`).

Note on naming: synthetic LTM variable columns use the U+205A "two dot punctuation" separator (`⁚`) and the U+2192 arrow (`→`), e.g. `$⁚ltm⁚link_score⁚pop[nyc]→$⁚ltm⁚agg⁚0`. Synthetic aggregate nodes are named `$⁚ltm⁚agg⁚N`.

## Phase A: Documentation prose review (the main human task)

The automated suite checks doc-link integrity but not prose correctness. A maintainer should read the doc diffs and confirm they accurately describe what shipped.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Read `git diff a397fe76..HEAD -- docs/design/ltm--loops-that-matter.md` | The new array material describes: the `model_ltm_reference_sites` IR (`ClassifiedSite` = reference shape + target element + `Direct`/`ThroughAgg` routing); `AggNode.read_slice` (`Pinned`/`Iterated`/`Reduced` axes) and `result_dims`; the consolidated `reducer_kind` table (`Linear`/`Nonlinear`/`Constant`); element-level A2A `Loop::stocks` and per-slot `loop_partitions`; iterated-dimension subscripts (`row_sum[D1]` inside `growth[D1,D2]`) classified as `Bare`; per-source-element link scores for disjoint-dim arrayed-to-arrayed targets plus the unscoreable-edge `Warning`; the analytic STDDEV partial; RANK's documented delta-ratio rationale; the budgeted cross-aggregate loop recovery and cyclic-ordering enumeration with `agg_recovery_truncated`; per-element graphical-function static link polarity. No stale references to `builtin_is_array_reducer`, `route_through_agg`, `enumerate_shapes`, or the retired per-shape `⁚wildcard`/`⁚dynamic` link scores. Section renumbering (if any) is internally consistent -- no dangling cross-references. |
| 2 | Read `git diff a397fe76..HEAD -- docs/reference/ltm--loops-that-matter.md` | The user-facing reference has a new arrayed-variables / aggregate-nodes section plus residual-limitation entries covering: arrayed loop scores per element; what the dynamic-index-reducer carve-out means for a modeler; RANK being approximate by design; the cross-aggregate-recovery truncation `Warning`; iterated/mapped-dimension handling. Tone matches the rest of the doc (modeler-facing, not engine-internal). |
| 3 | Read `git diff a397fe76..HEAD -- src/simlin-engine/CLAUDE.md` | "Last updated" date reflects the cluster; the Phases 1-8 summary is accurate; the module-map entries for `db_ltm_ir.rs`, `db_ltm_ir_tests.rs`, `ltm_agg.rs`, `ltm_augment.rs`, `db_ltm.rs`, `db_analysis.rs`, `ltm/polarity.rs` reflect the new behavior. No fabricated symbol names (e.g. there is no `Table::elements()` -- per-element graphical-function tables are the `tables: Vec<Table>` field on `Variable::Var`). |
| 4 | Read `git diff a397fe76..HEAD -- docs/architecture.md` | `db_ltm_ir.rs` appears in the enumerated list of `simlin-engine` salsa modules. |
| 5 | Skim the postscripts on `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` and `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md` | Each has a postscript noting what the arrays-hardening cluster changed; the before/after measurement numbers are internally consistent and any discrepancy with a prior re-measurement is explained. |
| 6 | Read `git diff a397fe76..HEAD -- docs/tech-debt.md` | Items #20 (conservative-slice carve-out closed by #514; trailing residual-carve-out clause), #21 (graphical-function monotonicity half resolved via #492/#502; residual `dy`-vs-slope nuance pointed at #536), #27 (STDDEV resolved via #483; RANK WONTFIX-by-design), #35 (resolved via #487) updated with dated resolution notes and refreshed "Last reviewed" dates; the follow-up items (#525, #526, #527, #528, #533, #534, #536) are filed (in epic #488 and/or referenced from tech-debt); no LTM-arrays residual concern silently dropped. |

## Phase B: GitHub issue-tracking check

| Step | Action | Expected |
|------|--------|----------|
| 1 | `for n in 520 487 511 510 514 515 483 502 492; do gh issue view $n --json state,title --jq '"\(.state)  #\(.title)"'; done` | All nine `CLOSED`. (Each was closed with a comment citing its implementing commit(s) and phase.) |
| 2 | `gh issue view 488` | The checklist boxes for #520, #487, #511, #510, #514, #515, #483, #502, #492 are all `[x]` with commit SHAs; remaining unchecked boxes are genuine deferred follow-ups (e.g. #525, #526, #527, #528, #533, #534, #536 and the older umbrella items). |
| 3 | `for n in 525 526 527 528 533 534 536; do gh issue view $n --json state,title --jq '"\(.state)  #\(.title)"'; done` | All exist (open) and describe the residual limitations the docs reference (PREVIOUS(SUM()) codegen for partial-iterated subscripts; mapped sliced reducers staying conservative; non-uniform-x graphical-function monotonicity nuance; etc.). |

## Phase C: End-to-end -- arrayed-LTM model file round-trip

Purpose: confirm a real model file (not a programmatically-built `TestProject`) with arrayed feedback gets sane loop detection and loop-score values through the same pipeline a modeler hits -- covers the integration the unit tests stub.

1. Author a small XMILE model `/tmp/ltm-verify/disjoint_a2a.stmx`:
   - `Region = {Boston, Denver, Austin}`; `pop[Region]` stock with heterogeneous inits, inflow `births[Region] = pop * 0.05` (a per-element reinforcing loop `pop[r] -> births[r] -> pop[r]`).
   - `Product = {Widget, Gadget}`; `widgets[Product]` stock, inflow `production[Product] = widgets * 0.03` (an independent per-element reinforcing loop over a *different* dimension).
   - No coupling between the two subsystems. Euler, `0..10`, `dt 1`.
2. `cargo run -p simlin-cli -- simulate --xmile /tmp/ltm-verify/disjoint_a2a.stmx --ltm > /tmp/ltm-verify/disjoint_a2a.tsv`

| Step | Action | Expected |
|------|--------|----------|
| 1 | The simulate command exits 0; the TSV header includes synthetic `$⁚ltm⁚...` columns. | No panic, no `"PREVIOUS requires a variable reference"` error. |
| 2 | If the CLI surfaces detected loops (or inspect via the engine `model_detected_loops`): count the loops. | Two reinforcing loops -- one over `Region` (3 elements), one over `Product` (2 elements) -- detected independently. |
| 3 | Inspect the relative loop scores per element (via `@simlin/serve`'s dominant-loop panel if built, or `simlin_analyze_get_relative_loop_score` with the subscripted loop-id form, or `compute_rel_loop_scores_per_element` through the engine API). | Each loop's relative score is `+1.0` at every element/step where it is active -- each loop is alone in its own per-element partition. **Pre-fix the two reinforcing loops would cross-normalize to ~0.5 each** (they pooled into a single partition). This is the AC2.1 regression class. |
| 4 | (Optional) Add to the same model an iterated-dimension variable: `row_val[Region] = pop[Region] * 0.001` consumed by `births` (so `births[Region] = pop * 0.05 + row_val[Region]`). Re-run with `--ltm`. | Still exits 0; the link score for `row_val -> births` is a bare A2A score (no element subscript broadcast surprises); the iterated-dim reference does not trip the partial codegen. This exercises AC3.1. |
| 5 | (Optional) Feed the same file through the `@simlin/mcp` stdio server's analyze-style tool. | Loop list and scores come back without error. |

## Phase D: End-to-end -- STDDEV reducer in a real model

Purpose: spot-check the analytic STDDEV per-element link-score partial (AC6.1) end-to-end through a model file rather than the programmatic fixture.

1. Author `/tmp/ltm-verify/stddev.stmx`: `Region = {a, b, c}`; `s[Region]` stock with heterogeneous inits (`a=10`, `b=20`, `c=30`), inflow `update[Region] = total * 0.01`; `total = STDDEV(s[*])` (so `total` feeds back into `s`). Euler, `0..10`, `dt 1`.
2. `cargo run -p simlin-cli -- simulate --xmile /tmp/ltm-verify/stddev.stmx --ltm > /tmp/ltm-verify/stddev.tsv`

| Step | Action | Expected |
|------|--------|----------|
| 1 | The simulate command exits 0. | A `$⁚ltm⁚link_score⁚s[a]→total` column (and `[b]`, `[c]`) is present in the header. |
| 2 | Inspect those three link-score columns over time. | Each is finite and **not uniformly `+1`** at every step -- the analytic population-variance partial varies with the spread of `s`. (Pre-#483 the STDDEV stand-in was a degenerate delta-ratio that collapsed to 1.) The exact values are already checked by `test_stddev_link_score_matches_hand_calc`; this is a "looks alive in a real model" sanity pass. |
| 3 | (Optional) Replace `update[Region] = total * 0.01` with `update[Region] = 0.5` (a constant, uniform per-element flow -- the "STDDEV-constant regime"). Re-run with `--ltm`. | The `s[d]→total` link scores are ~0 at every step (a uniform additive shift leaves the standard deviation unchanged, so `total` does not respond). This is the AC6.2 regression class. |

## Human Verification Required (recap)

| Criterion | Why manual | Steps |
|-----------|------------|-------|
| AC8.3 (doc content) | Prose correctness/consistency is not test-asserted; only link integrity is | Phase A steps 1-6 |
| AC8.4 (GitHub closures, epic checklist, tech-debt content) | `gh issue close`/`edit` are out-of-band actions; tech-debt prose is not test-asserted | Phase B steps 1-3; Phase A step 6 |
| AC8.2 (golden-diff rationale) | The `ltm_results.tsv` byte-check is automated and passes (no checked-in golden changed in the whole cluster); a human spot-checks that the few inline hand-calc constants the test files *did* change carry rationale in their commit messages | Read the commit messages of `11eb1af1` (Phase 2), `ec018d0b`/`bfb61d08`/`7f837928` (Phase 4), `b4eac795`/`2dbd658c` (Phase 5), `26ed48d3`/`dd838dde` (Phase 6) for per-test golden-diff notes |
| Interpretive resolutions | Confirm the resolutions recorded in `phase_05.md` / `phase_06.md` / `phase_07.md` match what shipped | Phase A step 1 (design-doc) cross-checked against the phase files |
| AC2.1 / AC3.1 / AC6.1 / AC6.2 end-to-end | The unit/integration tests build models programmatically; a model-file round-trip confirms the CLI/serve/MCP integration | Phases C and D |

## Traceability

| Acceptance criterion | Automated test(s) | Manual step |
|----------------------|-------------------|-------------|
| AC1.1-AC1.5 (#520 unified IR; `reducer_kind` consolidation; Phase-1 byte-identical) | `src/simlin-engine/src/db_ltm_ir.rs` + `db_ltm_ir_tests.rs`; `ltm_agg.rs` `mod tests` (`reducer_kind_classifies_every_array_reducer`, `*_reducer_is_not_hoisted`); `db_element_graph_tests.rs`; `tests/simulate_ltm.rs::simulates_population_ltm` vs `test/logistic_growth_ltm/ltm_results.tsv` | Phase A step 1, 3 |
| AC2.1-AC2.5 (#487 element-level A2A loops; per-slot `loop_partitions`; FFI) | `tests/simulate_ltm.rs::test_disconnected_a2a_loops_normalize_per_partition`, `test_independent_subsystems_partitioned_relative_scores`, `test_coupled_two_stock_single_partition`, `test_arms_race_single_partition`, `test_a2a_two_loop_relative_scores_sum_to_100`; `ltm/tests.rs::test_partition_for_loop_*`; `db_ltm_unified_tests.rs` (`a2a_loop_partitions_have_one_entry_per_element`, `cross_agg_two_petal_loops_match_pre_fix_content`, ...); `src/libsimlin/tests/analysis.rs::test_two_a2a_subsystems_per_slot_rel_score_round_trips`, `test_subscripted_loop_id_uses_per_element_cache`, `test_arrayed_bare_id_returns_argmax_abs_not_slot_zero` | Phase C steps 1-3 |
| AC3.1-AC3.5 (#511 iterated-dim subscript -> Bare; #510 disjoint-dim arrayed link scores; unscoreable-edge Warning; mapped-dim) | `db_ltm_ir_tests.rs` (`ir_iterated_dim_subscript_is_bare`, `ir_mapped_iterated_dim_subscript_is_bare`, `ir_position_mismatched_iterated_dim_stays_dynamic`, `ir_partially_iterated_dim_subscript_not_bare`); `db_element_graph_tests.rs` (`element_graph_iterated_dim_subscript_same_element_projection`, `element_graph_mapped_iterated_dim_matches_bare_baseline`); `ltm_augment.rs` `mod tests` (`test_iterated_dim_subscript_partial_is_bare`); `tests/simulate_ltm.rs` (`test_iterated_dim_subscript_link_score_is_bare_and_simulates`, `test_iterated_dim_subscript_loop_score_matches_hand_calc`, `test_disjoint_dim_arrayed_target_per_source_element_link_scores`, `test_disjoint_dim_unscoreable_edge_warns_and_emits_no_link_score`) | Phase C step 4 |
| AC4.1-AC4.5 (#514 hoist sliced reducer subexpressions; `read_slice`; retire Wildcard cross-product) | `ltm_agg.rs` `mod tests` (`slice_reducer_subexpression_is_hoisted`, `sliced_reducer_over_iterated_dim_mints_arrayed_agg`, `mixed_pinned_iterated_reduced_slice_mints_arrayed_agg`, `dynamic_index_reducer_subexpression_is_not_hoisted`); `db_element_graph_tests.rs` (`element_graph_sliced_reducer_reads_only_pinned_row`, `element_graph_arrayed_agg_over_iterated_dim`, `element_graph_dynamic_index_reducer_stays_conservative`); `db_ltm_unified_tests.rs` (`sliced_agg_link_scores_cover_only_the_read_rows`, `no_wildcard_or_dynamic_link_scores_for_reducer_models`); `tests/simulate_ltm.rs` (`test_sliced_agg_cross_element_loop_simulates`, `test_arrayed_sliced_agg_cross_element_loop_simulates`) | -- (no UI surface; AC4.5's `emit_edges_for_reference`-arm shape is the documented code-review check) |
| AC5.1-AC5.4 (#515 budgeted cross-agg loop recovery; cyclic-ordering enumeration; truncation flag + Warning) | `db_ltm_unified_tests.rs` (`cross_agg_loop_recovery_truncates_at_budget`, `cross_agg_loop_recovery_four_petals_enumerates_cyclic_orderings`, `cross_agg_loop_recovery_three_petals_no_truncation`, `cross_element_loop_through_agg_is_recovered`, `cross_agg_two_petal_loops_match_pre_fix_content`); `db_ltm_tests.rs::cyclic_orderings_enumerates_distinct_rotation_and_mirror_classes`; `tests/simulate_ltm.rs::test_four_petal_cyclic_orderings_share_loop_score_series` | -- |
| AC6.1-AC6.5 (#483 analytic STDDEV partial; RANK keeps documented delta-ratio; SUM/MEAN/MIN/MAX unchanged) | `ltm_augment.rs` `mod tests` (`test_generate_stddev_equation`, `test_generate_stddev_single_element_is_zero`, `test_generate_rank_keeps_delta_ratio`, `test_generate_{sum,mean,min,max}_equation`, `test_classify_reducer_*`); `tests/simulate_ltm.rs` (`test_stddev_link_score_matches_hand_calc`, `test_stddev_invariant_regime_link_scores_zero`) | Phase D steps 1-3 |
| AC7.1-AC7.5 (#502 per-element graphical-function static link polarity; #492 y-range-relative monotonicity epsilon) | `ltm/tests.rs` (`test_per_element_gf_link_polarity_agree`, `test_per_element_gf_link_polarity_disagree`, `test_per_element_gf_link_polarity_fixed_index`, `test_per_element_gf_link_polarity_one_nonmonotone_element_ignored`, `test_graphical_function_polarity`, `test_graphical_function_polarity_tolerates_import_noise`, `test_lookup_forward_backward_arm_polarity`) | -- (per-element GF polarity has no distinct UI surface beyond the loop-polarity display) |
| AC8.1 (`cargo test --workspace` green within the 3-minute cap; pre-commit passes each commit) | operational -- all suites green; per-test wall times sub-second | Prerequisites |
| AC8.2 (Phase-1 golden byte-unchanged; behavior-changing diffs explained) | `git diff a397fe76..HEAD -- test/` empty; `tests/simulate_ltm.rs::simulates_population_ltm` | Phase A step 6 + the commit-message review noted above |
| AC8.3 (CLAUDE.md + design/reference LTM docs updated; link integrity) | `python3 scripts/check-docs.py` | Phase A steps 1-5 |
| AC8.4 (GH issues closed; epic #488 ticked; tech-debt updated) | `gh issue view ...`; `git diff -- docs/tech-debt.md` | Phase B; Phase A step 6 |
