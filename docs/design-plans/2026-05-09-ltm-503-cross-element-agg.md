# LTM Cross-Element Aggregate Scoring Design

Date: 2026-05-09
Resolves: GH **#503** (cross-element loops normalize by diagonal A2A link
score instead of Δ-aggregate). Related: GH **#488** (LTM epic), **#487**
(A2A `partition = None`), `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`.

## Summary

Simlin's "Loops That Matter" (LTM) subsystem instruments a model with synthetic auxiliary variables that compute, per timestep, each causal link's contribution to its target's change (the link score) and each feedback loop's overall strength (the loop score, later normalized into a relative loop score). For arrayed (subscripted) variables this is done on an element-level causal graph so loops are found and scored at element granularity. This change fixes that element-level path, which today produces meaningless scores for any feedback that crosses element boundaries.

There are three layered defects. (1) When a link's target is a per-element-equation array (`Ast::Arrayed` -- the `<element subscript="...">` form, e.g. `mp[NYC] = (pop[NYC] - pop[Boston]) * c`), the ceteris-paribus partial-equation generator doesn't recognize that AST shape and falls through to a `"0"` placeholder, so the link score is a nonzero but garbage number. (2) A loop that genuinely traverses distinct elements (`pop[nyc] → mp[boston] → mi[nyc] → pop[nyc]`) is collapsed to a scalar `Loop` whose loop-score equation references the *diagonal* apply-to-all link scores (`pop[d]→mp[d]`), not the element slots the loop actually visits. (3) A cross-element loop that runs through an inlined array reducer (`share[r] = pop[r] / SUM(pop[*])`) is scored by a single lumped link score -- "how much of `Δshare[r]` came from the whole `SUM` moving" -- which omits each source element's fractional contribution to the aggregate's velocity, exactly the factor that matters when elements have very different magnitudes.

The fix: (A) build proper per-element `Equation::Arrayed` partials by running the existing per-reference-shape `PREVIOUS`-wrapping machinery on each element's own expression; (B) keep element subscripts on cross-element loops and have the loop-score equation reference subscripted link scores along the actual path; (C) align the discovery-mode (strongest-path) graph parser with the new naming so it traverses the same correct graph; and (D) -- the conceptual core -- treat each maximal inlined reducer subexpression as an implicit aggregate node (`$⁚ltm⁚agg⁚{n}`), mirroring how the published LTM papers handle macros like `DELAY3`/`SMOOTH`: the aggregation has hidden internal structure, so route causality through it (`pop[d] → agg → share[r]`), score the two halves with real per-element link scores, compose them by the chain rule, and trim the agg node when *reporting* the loop. With reducers handled as aggregate nodes, the ad-hoc `⁚wildcard`/`⁚dynamic` link-score variants and the `shape_aware_source_ref` Wildcard/DynamicIndex approximation become dead code and are removed. Scalar and pure-A2A models are untouched (no reducers, no per-element-equation targets => no aggregate nodes => no element-graph change).

## Definition of Done

- Link scores into per-element-equation (`Ast::Arrayed`) targets carry
  meaningful per-element partials derived from each element's own
  equation, not a `"0"`-derived placeholder.
- Cross-element feedback loops -- whether they arise from fixed-index
  references (`mp[NYC] = (pop[NYC] - pop[Boston]) * c`) or from
  wildcard/dynamic array reducers (`share[r] = pop[r] / SUM(pop[*])`) --
  are scored using the actual per-element link scores along the loop's
  element-level path, not a scalar collapse onto diagonal A2A link-score
  values.
- Inlined array reducers that participate in feedback (`SUM`, `MEAN`,
  `STDDEV`, `MIN`/`MAX`, `RANK`, partial reduces like `SUM(m[D1,*])`,
  nested reducers) are treated as implicit aggregate nodes; cross-element
  coupling through a reducer composes by the chain rule (path scores), so
  models with heterogeneous element magnitudes are scored correctly.
- The `⁚wildcard` / `⁚dynamic` link-score variants and the
  `shape_aware_source_ref` Wildcard/DynamicIndex approximation are
  removed (obviated by the aggregate-node treatment).
- Discovery mode (strongest-path) produces correct cross-element loops on
  the same models, because the element graph it traverses is now correct.
- Existing scalar and pure-A2A model results are unchanged (no reducers =>
  no aggregate nodes => no element-graph change).
- Docs updated (`docs/design/ltm--loops-that-matter.md`,
  `src/simlin-engine/CLAUDE.md`, the `2026-04-25-ltm-per-ref-elem-graph.md`
  measurement postscript); GH #503 marked resolved and epic #488's
  checklist ticked.
- `cargo test --workspace` passes within the 3-minute cap; the pre-commit
  hook passes.

## Acceptance Criteria

### ltm-503-cross-element-agg.AC1: Arrayed-target link scores carry real per-element partials

- **ltm-503-cross-element-agg.AC1.1 Success:** For a per-element-equation aux `mp[NYC] = (pop[NYC] - pop[Boston]) * 0.01`, `mp[Boston] = (pop[Boston] - pop[NYC]) * 0.01`, the link score `$⁚ltm⁚link_score⁚population[nyc]→migration_pressure` is an `Equation::Arrayed` over the target dimension whose `nyc` slot partial is (the canonical form of) `(pop[nyc] - PREVIOUS(pop[boston])) * 0.01` and whose `boston` slot partial is `(PREVIOUS(pop[boston]) - pop[nyc]) * 0.01` -- no `"0"` placeholder.
- **ltm-503-cross-element-agg.AC1.2 Success:** For the same model, `$⁚ltm⁚link_score⁚population[boston]→migration_pressure` is `Equation::Arrayed`; `nyc` slot partial `(PREVIOUS(pop[nyc]) - pop[boston]) * 0.01`, `boston` slot partial `(pop[boston] - PREVIOUS(pop[nyc])) * 0.01`.
- **ltm-503-cross-element-agg.AC1.3 Success:** A stock-to-flow link score into a per-element-equation arrayed flow yields per-element partials referencing the flow's actual equation contents, not `"0"` (regression sibling to `test_stock_to_flow_link_score_handles_apply_to_all`).
- **ltm-503-cross-element-agg.AC1.4 Success:** In the `cross_element_ltm` fixture simulation, `$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in` has magnitude approximately 1 in the NYC slot at every step >= 2 (since `migration_in[NYC] = MAX(-migration_pressure[Boston], 0)` and `migration_pressure[Boston] < 0` throughout) and is identically 0 in the Boston slot. Pre-fix this slot carried a `"0"`-partial-derived value far from 1.

### ltm-503-cross-element-agg.AC2: Cross-element loops scored element-level (exhaustive path)

- **ltm-503-cross-element-agg.AC2.1 Success:** In the `cross_element_ltm` fixture, the loop `population[nyc] → migration_pressure[boston] → migration_in[nyc] → population[nyc]` is enumerated, and its `loop_score` equation references `"$⁚ltm⁚link_score⁚population[nyc]→migration_pressure"[boston]`, `"$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in"[nyc]`, and `"$⁚ltm⁚link_score⁚migration_in→population"[nyc]` -- each subscripted at the element the loop visits -- not the unsubscripted A2A diagonal names.
- **ltm-503-cross-element-agg.AC2.2 Success:** That loop's `loop_score` series matches a hand calculation at >= 1 timestep (within 1e-6).
- **ltm-503-cross-element-agg.AC2.3 Success:** The symmetric loop `population[boston] → migration_pressure[nyc] → migration_in[boston] → population[boston]` is also enumerated with the analogous subscripted references.
- **ltm-503-cross-element-agg.AC2.4 Success:** The pure-A2A loop `population[r] → births[r] → population[r]` remains a single A2A `loop_score` variable with per-element slots (no regression from the cross-element changes).
- **ltm-503-cross-element-agg.AC2.5 Edge:** A model with no arrayed variables has its loops unchanged -- their `loop_score` equations reference unsubscripted scalar link scores exactly as before.

### ltm-503-cross-element-agg.AC3: Discovery mode finds cross-element loops

- **ltm-503-cross-element-agg.AC3.1 Success:** `discover_loops_element_level` on the `cross_element_ltm` fixture finds a loop whose links include `population[nyc] → migration_pressure[boston]` (or the symmetric `population[boston] → migration_pressure[nyc]`), not merely "some subscripted loop".
- **ltm-503-cross-element-agg.AC3.2 Success:** On a model that factors out a scalar reducer (`total_pop = SUM(pop[*])`, `migration[r] = total_pop * c - pop[r] * c`, `pop[r]` stock fed by `migration[r]`), discovery finds the loop `pop[*] → total_pop → migration[r] → pop[r]` -- i.e. the scalar->arrayed link score's element slots resolve to `(total_pop, migration[d])` edges, not `(total_pop[d], migration[d])`. (Pre-fix this loop was silently undiscoverable.)
- **ltm-503-cross-element-agg.AC3.3 Success:** A `parse_link_offsets` unit test: a scalar->arrayed link score named `$⁚ltm⁚link_score⁚total_pop→migration[nyc]` resolves to the edge `(total_pop, migration[nyc])`.

### ltm-503-cross-element-agg.AC4: Inlined reducers become aggregate nodes

- **ltm-503-cross-element-agg.AC4.1 Success:** For `share[r] = pop[r] / SUM(pop[*])` with `share` feeding back into `pop`, `model_ltm_variables` emits a synthetic aux `$⁚ltm⁚agg⁚0` with equation `SUM(pop[*])`; the element graph contains `pop[d] → $⁚ltm⁚agg⁚0` for every element `d` and `$⁚ltm⁚agg⁚0 → share[r]` for every element `r`, and contains no direct `pop[d] → share[e]` Wildcard-derived edges.
- **ltm-503-cross-element-agg.AC4.2 Success:** A cross-element feedback loop through the aggregate is enumerated traversing `$⁚ltm⁚agg⁚0` twice; the reported `Loop` has the agg node trimmed (its links are `pop[d] → share[e]` style); its `loop_score` equation is the product of the actual per-element link scores along the un-trimmed path, including the `pop[d]→agg` and `agg→share[e]` halves.
- **ltm-503-cross-element-agg.AC4.3 Success:** A variable whose entire dt-equation is exactly one reducer call (`total_population = SUM(population[*])`) mints no synthetic agg node; `total_population` itself is the aggregate node, and behavior is otherwise as in AC4.1/4.2.
- **ltm-503-cross-element-agg.AC4.4 Success:** Nested reducers (`x = SUM(a[*]) / SUM(b[*])`) mint two distinct agg nodes; two textually-distinct-but-AST-identical reducer subexpressions within a model dedupe to one agg node.
- **ltm-503-cross-element-agg.AC4.5 Success (heterogeneous magnitudes -- the issue's motivating case):** For a 2-region `share[r] = pop[r] / SUM(pop[*])` model with `pop[big] >> pop[small]`, the relative loop scores of the cross-element loops differ measurably from what a single lumped `|Δ_aggregate(share[r]) / Δshare[r]|` link score would give, and match a hand calculation -- the per-element `|Δpop[d] / Δagg|` factor is present and non-constant across `d`.
- **ltm-503-cross-element-agg.AC4.6 Success (partial reduce):** For an agg node `agg[D1] = SUM(matrix[D1,*])` arising from hoisting `SUM(matrix[d1,*])` inside a target, the agg node is itself arrayed over `D1`, the reducer link-score machinery emits per-element link scores for the `matrix[d1,d2] → agg[d1]` edges, and a cross-element loop over the reduced axis is scored using them.
- **ltm-503-cross-element-agg.AC4.7 Success:** Discovery mode on the AC4.1 (inlined-reducer) model finds the cross-element-through-agg loop (the rerouted graph makes it reachable).

### ltm-503-cross-element-agg.AC5: Wildcard/Dynamic link-score path retired

- **ltm-503-cross-element-agg.AC5.1 Success:** No `$⁚ltm⁚link_score⁚…⁚wildcard` or `…⁚dynamic` variables are emitted for any model; `link_score_var_name` no longer appends those suffixes; `parse_link_offsets` no longer strips them; `resolve_link_score_name_for_loop` has no `Wildcard`/`DynamicIndex` cases.
- **ltm-503-cross-element-agg.AC5.2 Success:** `shape_aware_source_ref`'s Wildcard/DynamicIndex TODO branch is removed (the function's only remaining special case, if any, is `FixedIndex`).

### ltm-503-cross-element-agg.AC6: No regression on scalar / pure-A2A models; perf budget

- **ltm-503-cross-element-agg.AC6.1 Success:** Golden-data integration tests for scalar and pure-A2A models (e.g. `simulates_population_ltm` vs `test/logistic_growth_ltm/ltm_results.tsv`, the WRLD3 LTM smoke) pass with unchanged expected values.
- **ltm-503-cross-element-agg.AC6.2 Success:** `cargo test --workspace` completes within the 3-minute wall-clock cap; the pre-commit hook passes.

### ltm-503-cross-element-agg.AC7: Docs and tracking updated

- **ltm-503-cross-element-agg.AC7.1 Success:** `docs/design/ltm--loops-that-matter.md` describes the aggregate-node treatment and element-level cross-element loop scoring; `src/simlin-engine/CLAUDE.md` reflects the new `$⁚ltm⁚agg⁚{n}` synthetic family, the retired Wildcard path, and the `LtmSyntheticVar.equation` type change; the `2026-04-25-ltm-per-ref-elem-graph.md` measurement postscript notes the SCC/loop-count shift on reducer-bearing fixtures.
- **ltm-503-cross-element-agg.AC7.2 Success:** GH #503 is marked resolved referencing the implementing commit(s); epic #488's checklist is ticked.

## Glossary

- **LTM (Loops That Matter)**: A feedback-loop-dominance analysis method (Eberlein & Schoenberg 2020 and follow-ups) that quantifies, at each point in simulated time, how much each causal link and each feedback loop drives the model's behavior. Simlin implements it by adding synthetic "score" variables to the model; see `docs/design/ltm--loops-that-matter.md`.
- **link score**: Per-timestep measure of how much a single causal link `x → z` contributed to `z`'s change, computed ceteris paribus (re-evaluate `z`'s equation with `x` at its current value and every other input held at its `PREVIOUS()` value), as a signed ratio in roughly [-1, 1].
- **loop score**: Per-timestep strength of a feedback loop, the product of the link scores around it. Materialized as a synthetic variable `$⁚ltm⁚loop_score⁚{loop_id}`.
- **relative loop score**: A loop's score normalized against the sum of absolute loop scores within its cycle partition, so loops are only compared to structurally related loops. Computed post-simulation (`ltm_post::compute_rel_loop_scores`), not synthesized.
- **cross-element loop**: A feedback loop through an arrayed variable that visits *different* elements at different points (`pop[nyc] → mp[boston] → mi[nyc] → pop[nyc]`), as opposed to staying on one element. The subject of this fix.
- **A2A (apply-to-all)**: An arrayed equation with one formula evaluated for every element of its dimension(s) -- `Equation::ApplyToAll` / `Ast::ApplyToAll`. A "pure-A2A loop" like `pop[r] → births[r] → pop[r]` stays on a single element index `r` and is one loop-score variable with per-element slots.
- **`Ast::Arrayed` vs `Ast::ApplyToAll`**: Two representations of an arrayed equation. `ApplyToAll` is one shared formula over the dimension; `Arrayed` is the per-element form (XMILE `<element subscript="...">`) with a distinct expression per element key plus an optional default. Today the link-score partial generator handles `ApplyToAll` but not `Arrayed` (it returns `"0"`).
- **reducer / array reducer**: A builtin that aggregates over an array dimension to a smaller result: `SUM`, `MEAN`, `STDDEV`, `MIN`/`MAX`, `RANK`, `SIZE`. When written inline in another variable's equation (`SUM(pop[*])`) it has no node of its own today.
- **partial reduce**: A reducer that collapses only some of a source's axes, leaving an arrayed result -- e.g. `SUM(matrix[D1,*])` reduces the second axis and yields an array over `D1`. Phase 4 adds support for this in the reducer link-score machinery.
- **ceteris paribus / partial change**: "All else equal." The LTM partial equation evaluates a target's formula with exactly one input varying and all others frozen at their previous values; the term "partial" here is the discrete analogue of a partial derivative.
- **aggregate node / hoist (the "C-hoist")**: This design's central move -- treat a maximal inlined reducer subexpression as an implicit synthetic auxiliary (`$⁚ltm⁚agg⁚{n}`, "the aggregate") so causality routes `source[d] → agg → target`, both halves get real per-element link scores, and they compose by the chain rule. The agg node is *trimmed* from the loop when it is reported (the path `pop[d] → agg → share[e]` is shown as `pop[d] → share[e]`), exactly as the LTM papers trim a macro's hidden internal nodes.
- **`$⁚ltm⁚...` synthetic-variable naming convention**: All LTM-generated variables use a `$` prefix and U+205A (`⁚`, TWO DOT PUNCTUATION) as separator -- e.g. `$⁚ltm⁚link_score⁚{from}→{to}`, `$⁚ltm⁚loop_score⁚{loop_id}`, and the new `$⁚ltm⁚agg⁚{n}`. The `$` avoids collisions with user variables; `⁚` is a valid identifier character but essentially never appears in authored equations. In generated equations these names are double-quoted, and a `[elem]` subscript may follow.
- **`RefShape` (Bare / FixedIndex / Wildcard / DynamicIndex)**: Per-reference-site classification of how a source variable is accessed in a target's AST, introduced by the 2026-04-25 per-ref element-graph work. `Bare` = a plain `Var` reference (scalar or same-element A2A); `FixedIndex` = literal subscripts like `x[NYC]` (broadcast edges); `Wildcard` = a reducer access like `x[*]`; `DynamicIndex` = non-literal index (`@N`, `Range`, arbitrary expression), handled conservatively. This change reroutes the `Wildcard`/`DynamicIndex` cases through aggregate nodes and retires the corresponding link-score variants.
- **strongest-path / discovery mode**: An alternative LTM mode (`ltm_discovery_mode`) for models too large for exhaustive loop enumeration: link scores are emitted for *all* edges, and after simulation a heuristic strongest-path DFS over a per-timestep `SearchGraph` (built by parsing link-score variable names via `parse_link_offsets`) finds the dominant loops. Must be kept consistent with the exhaustive path's graph and naming.
- **Johnson's algorithm**: The classic algorithm for enumerating all elementary circuits (simple cycles) of a directed graph; Simlin runs it on the variable-level and (for cross-element/mixed cycles) element-level causal graphs to find loops exhaustively.
- **cycle partition**: A group of stocks connected by feedback (a strongly connected component of the stock-to-stock reachability graph). Relative loop scores are normalized within a partition; module-internal and element-level stocks are folded in so partition assignment stays correct. The `partition_for_loop` lookup keys a loop to its partition.
- **salsa (incremental compilation)**: The incremental-computation framework Simlin's compiler is built on; LTM is a set of "tracked functions" (`model_ltm_variables`, `model_element_causal_edges`, etc.) that re-run only when their inputs change. The new `enumerate_agg_nodes` helper must be a deterministic, salsa-friendly function shared by the element-graph and LTM-variable tracked functions.

## Architecture

### Background: what's broken

Three layered defects in the cross-element / arrayed-target LTM path:

1. **Arrayed-target partials collapse to `"0"`.** `generate_auxiliary_to_auxiliary_equation` and `generate_stock_to_flow_equation` (`src/simlin-engine/src/ltm_augment.rs`) read the target's AST and only match `Ast::Scalar | Ast::ApplyToAll`; a per-element-equation target (`Ast::Arrayed`, the `<element subscript="...">` form) falls through to `_ => "0"`, so every link score into such a target has partial equation `"0"` and computes a nonzero-but-meaningless value. There is already an analogous (now-fixed) regression test, `test_stock_to_flow_link_score_handles_apply_to_all`, for the `ApplyToAll` variant of this same fall-through; `Arrayed` was never added. The `cross_element_ltm` fixture's `migration_pressure` and `migration_in` are per-element-equation, so its cross-element loops are scored from garbage today.

2. **Cross-element loops collapse to a scalar `Loop` with diagonal A2A link names.** `build_element_level_loops` (`src/simlin-engine/src/db_ltm.rs`)'s `is_cross_element` branch strips subscripts off the element-level circuit, builds a scalar `Loop` (`dimensions: vec![]`) whose links are variable-level names, and `generate_loop_score_equation` references the canonical Bare A2A link scores. A loop that genuinely traverses `pop[nyc] -> mp[boston] -> mi[nyc] -> pop[nyc]` gets scored with `pop->mp`'s *diagonal* (`pop[d] -> mp[d]`) link scores, and a scalar loop-score equation reading an A2A link-score variable resolves to one arbitrary slot. (This is the latent issue noted in `docs/tech-debt.md`'s item-#33-adjacent description: "the loop_score equation `"link_score⁚A→B" * ...` compiled with every link_score reference treated as a scalar (slot 0 only)" -- subscripting the references fixes it cleanly.)

3. **Wildcard reducers are scored by a lumped link score.** `share[r] = pop[r] / SUM(pop[*])` emits all-pairs `pop[d] -> share[e]` element edges (Wildcard shape) and a single A2A-over-target-dim `…→share⁚wildcard` link score whose magnitude is `|Δ_aggregate(share[r]) / Δshare[r]|` -- "how much of Δshare[r] came from the whole SUM moving", not "from `pop[d]` specifically". The principled per-element quantity is the path score `pop[d] -> ⟨aggregate⟩ -> share[r]` = `LS(⟨aggregate⟩ -> share[r]) · |Δpop[d] / Δ⟨aggregate⟩|`; the second factor (element `d`'s fractional contribution to the aggregate's velocity) is exactly what differs under heterogeneous element magnitudes and is missing today. The `shape_aware_source_ref` TODO (`ltm_augment.rs`) documents the symptom.

### The fix

**(A) Arrayed-target partial equations.** When the target is `Ast::Arrayed`, the link-score equation generator builds an `Equation::Arrayed`: for each `(element_key, expr)` in the target's per-element map, it derives that element's partial by applying the existing per-reference-shape `PREVIOUS`-wrapping (`build_partial_equation_shaped` / `wrap_non_matching_in_previous`) to `expr`, then assembles an arrayed equation with the same keys (and the same default-equation handling). `LtmSyntheticVar.equation` changes type from `String` to `datamodel::Equation` so this is representable; downstream `parse_ltm_equation` already takes `datamodel::Equation`, so the blast radius is the struct and its constructors. Bare-A2A and scalar link scores stay `Scalar`/`ApplyToAll` exactly as today.

**(B) Element-level cross-element loops.** `build_element_level_loops`'s `is_cross_element` branch stops collapsing to scalar/variable-level names; it keeps the element subscripts on each `Link` (the existing "mixed" branch already does element-level link construction -- generalize and reuse). `generate_loop_score_equation` emits *subscripted* link-score references: for a loop edge whose `Link.to` carries element subscript `[e]` and whose resolved link-score variable is dimensioned, the loop-score equation references `"$⁚ltm⁚link_score⁚{from-name}→{to-name}"[e]`. The `from-name` comes from `resolve_link_score_name_for_loop` (which already picks `from[elem]→to` for FixedIndex sources vs `from→to` for Bare); a new `target_element` parameter on that function selects the slot. The loop-score *variable* itself stays `Equation::Scalar` -- a cross-element loop visits fixed elements, it is not parameterized by a free dimension. The old "cross-element collapses to Bare diagonal" code path is deleted. Pure-A2A loops (`pop[r] -> births[r] -> pop[r]`) are unaffected -- they remain a single A2A loop-score variable with per-element slots.

**(C) Discovery-side alignment.** Verified: `parse_link_offsets` (`src/simlin-engine/src/ltm_finding.rs`) already expands a FixedIndex-A2A link score (`pop[nyc]→mp` with `dimensions = ["Region"]`) into `(pop[nyc], mp[d])` entries over the target dimension via `expand_fixed_from_a2a_link_offsets` -- so once (A) makes those values meaningful, discovery's FixedIndex cross-element edges are correct with no parser change. The one real gap: `expand_a2a_link_offsets` subscripts *both* sides of a Bare-A2A link score, which is wrong for a scalar-source -> arrayed-target link score (it invents a `total_pop[nyc]` node that doesn't match the unsubscripted `total_pop` node from the `pop[d]→total_pop` edges). Fix: emit scalar->arrayed link scores under a per-target-element name `{from}→{to}[{elem}]` (mirroring the existing arrayed->scalar `{from}[{elem}]→{to}` convention), so the source side is unambiguous, and the Bare-A2A expander subscripts both sides only when both are arrayed. This incidentally repairs a pre-existing discovery gap for scalar-source-in-a-loop models (e.g. the loop in `test_mixed_loop_scalar_per_element_scores`).

**(D) Reducers become aggregate nodes (the C-hoist).** A new LTM-only synthetic family `$⁚ltm⁚agg⁚{n}`: a normal computed auxiliary whose equation is one *maximal reducer subexpression* (`SUM(pop[*])`, `MEAN(...)`, `SUM(m[D1,*])`, ...) found in any LTM-relevant equation. A variable whose entire dt-equation is exactly one reducer call (`total_population = SUM(population[*])`) is its *own* aggregate node -- no synthetic minted. Naming is deterministic via a shared helper `enumerate_agg_nodes(model) -> Map<canonical_reducer_subexpr, agg_ident>` (canonicalization via parsed-AST equality, so whitespace/casing don't matter; identical subexpressions dedupe). The helper is consumed by **both**:
- `model_element_causal_edges` (`src/simlin-engine/src/db_analysis.rs`): when its reference-site walker encounters a wildcard/dynamic reducer reference inside a target, it reroutes -- instead of all-pairs `pop[d] -> target[e]` it emits `pop[d] -> agg` (arrayed source -> scalar/arrayed agg) and `agg -> target[e]` (agg -> arrayed target). For an *arrayed-result* reducer (`SUM(m[D1,*])` -> `agg[D1]`), the agg is itself arrayed over the un-reduced axes and the edges are emitted per element accordingly.
- `model_ltm_variables` (`src/simlin-engine/src/db_ltm.rs`): emits the `$⁚ltm⁚agg⁚{n}` auxiliaries (so the aggregate is *computed* during simulation and `PREVIOUS(agg)` is available) and the two link-score families -- `pop[d] -> agg` via the existing reducer link-score machinery (`try_cross_dimensional_link_scores` / `generate_element_to_scalar_equation` / `classify_reducer`, extended in this work to handle arrayed-result reducers), and `agg -> target` as a Bare scalar->arrayed (or A2A) link score.

LTM partial-equation building textually substitutes a recognized reducer subexpression with its agg name (so the link score `agg -> target[r]` holds bare-`pop[r]` at `PREVIOUS` and `agg` live -- exactly the existing Wildcard semantics, now correctly attributed to the aggregate). **Model equations are not rewritten** -- the element graph and the LTM partial equations reference `agg`; the simulation evaluates the inline reducer (the agg aux yields the same value).

Loop enumeration now sees `agg` as a real node; a cross-element loop genuinely traverses `pop[nyc] -> agg -> share[boston] -> ... -> pop[boston] -> agg -> share[nyc] -> ... -> pop[nyc]` with `agg` appearing twice; the loop score is the honest product of the Eq.-2 link scores along it. **Loop *reporting* trims agg nodes** -- the displayed `Loop` collapses `pop[d] -> agg -> share[e]` back to `pop[d] -> share[e]` (the way the 2020.1 paper trims internal macro nodes; a loop entirely internal to one agg, which `SUM` can't produce but a pathological reducer could, is dropped). Discovery benefits automatically (its graph is now correct). The diagonal conflation (`share[r] = pop[r]/SUM(pop[*])`: is the `pop[r] -> share[r]` link score the numerator effect, the SUM effect, or both?) is resolved by construction: the numerator path and the SUM path become *distinct loops*, scored separately -- the LTM-correct treatment of a source appearing on two causal paths.

**(E) Cleanup.** With (D) in place, the `Wildcard`/`DynamicIndex` link-score *variants* (`link_score_var_name`'s `⁚wildcard`/`⁚dynamic` suffixes, `parse_link_offsets`'s suffix stripping, `resolve_link_score_name_for_loop`'s Wildcard/DynamicIndex cases) and `shape_aware_source_ref`'s Wildcard/DynamicIndex TODO branch are dead and removed. The reference-site walker may still use `RefShape::Wildcard`/`DynamicIndex` internally as transient "this is a reducer reference, route through agg" markers; whether the enum variants survive is an implementation detail.

### Data flow

```
model variables --> enumerate_agg_nodes (shared helper, deterministic)
                         |
        +----------------+-----------------+
        v                                  v
model_element_causal_edges            model_ltm_variables
  reroutes reducer refs through         emits $⁚ltm⁚agg⁚n auxes
  agg nodes; element graph has          + pop[d]->agg link scores
  pop[d]->agg, agg->target[e]           + agg->target link scores
        |                               + per-shape link scores (A,B)
        v                               + loop_score vars referencing
build_loops_from_tiered                   subscripted link scores (B)
  / build_element_level_loops                 |
  element-level cross-element loops            |
  (Link.to subscripted), agg trimmed           v
  for reporting                          compile + simulate
        |                                      |
        +-----------------> Loop list <--------+
                                |
              exhaustive: loop_score var values
              discovery:  parse_link_offsets (C) -> SearchGraph
                          -> strongest-path -> FoundLoop
```

## Existing Patterns

This design follows established LTM patterns rather than introducing new ones:

- **Reducer link scores already exist for arrayed-source -> scalar-target.** `try_cross_dimensional_link_scores` + `generate_element_to_scalar_equation` + `classify_reducer` (`ltm_augment.rs` / `db_ltm.rs`) already compute, per source element, "how much varying that one element affects the scalar target while holding the others at `PREVIOUS`". (D) reuses this for `pop[d] -> agg` and extends it to arrayed-result reducers. It is also exactly the form `parse_link_offsets`'s `from[elem]→to` branch already parses.

- **Macros are already treated as expandable internal structure.** The 2020.1 paper ("Seamlessly Integrating LTM") handles `DELAY3`/`SMOOTH` by computing loop scores through the macro's hidden internal stocks/flows and trimming internal nodes for reporting; Simlin's module-pathway machinery (`enumerate_module_pathways`, composite scores) mirrors this. (D) applies the same idea to array reducers (which are morally macros: hidden aggregation structure).

- **Per-reference access shapes are already first-class.** The `2026-04-25-ltm-per-ref-elem-graph.md` work added `RefShape` (`Bare`/`FixedIndex`/`Wildcard`/`DynamicIndex`) and the per-element AST walker (`collect_reference_sites`/`collect_reference_shapes` in `db_analysis.rs`). (A) feeds the existing `build_partial_equation_shaped` machinery one per-element target expression at a time; (D) reroutes the `Wildcard`/`DynamicIndex` cases through agg nodes.

- **Synthetic LTM variables that other equations reference already exist** (stdlib-module composite vars, e.g. `module·$⁚ltm⁚composite⁚port`). `$⁚ltm⁚agg⁚n` is the first such reference target for *user* equations, but the mechanism (a salsa-tracked function mints synthetic auxes; other LTM equations reference them by name; the fragment compiler resolves them) is unchanged.

- **Element-subscripted `Link.from` strings** are already the encoding for cross-dimensional edges (`"pop[nyc]"` -- see `src/simlin-engine/CLAUDE.md` on `build_element_level_loops`). (B) extends the same convention to `Link.to` for cross-element loops and adds subscript-aware reference in the loop-score equation.

Divergence from the literature: array reducers are not discussed in any LTM paper (the 2023 "Improving LTM" correction addresses flow aggregation, not array-dimension reduction). Treating an inlined reducer as an aggregate node is a Simlin-specific extension, justified as the natural application of the published macro treatment.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Arrayed-target partial equations

**Goal:** Link scores into per-element-equation (`Ast::Arrayed`) targets carry real per-element partials, not a `"0"` placeholder.

**Components:**
- `generate_auxiliary_to_auxiliary_equation`, `generate_stock_to_flow_equation` in `src/simlin-engine/src/ltm_augment.rs` -- handle `Ast::Arrayed` targets by iterating per-element expressions and applying `build_partial_equation_shaped` per element.
- A new helper (in `ltm_augment.rs`) that assembles an `Equation::Arrayed` link-score equation from per-element partials + the target's default equation.
- `LtmSyntheticVar` in `src/simlin-engine/src/db.rs` -- `equation` field type changes `String -> datamodel::Equation`; update constructors in `db_ltm.rs` (`link_score_equation_text_shaped`, `try_cross_dimensional_link_scores`, the agg/loop emitters) and any callers that read `.equation` as a string.
- `parse_ltm_equation` / `compile_ltm_var_fragment` in `db_ltm.rs` -- confirm they accept an `Arrayed` link-score equation (they already take `datamodel::Equation` for model vars; verify the LTM path).

**Dependencies:** None (first phase).

**Done when:** Tests covering `ltm-503-cross-element-agg.AC1.1`-`AC1.4` pass (per-element partials are correct for per-element-equation targets, both aux-to-aux and stock-to-flow; the `cross_element_ltm` fixture's `migration_pressure[boston]→migration_in` link score is ~1 in the NYC slot and 0 in the Boston slot); `cargo test --workspace` is green.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Element-level cross-element loops (exhaustive path)

**Goal:** Cross-element feedback loops are scored from the actual per-element link scores along their element-level path.

**Components:**
- `build_element_level_loops` in `src/simlin-engine/src/db_ltm.rs` -- the `is_cross_element` branch keeps element subscripts on each `Link` (reuse/generalize the existing "mixed" branch's per-link construction); delete the "collapse to Bare diagonal scalar loop" code path and the now-unused unique-cycle-stripping logic for that case.
- `resolve_link_score_name_for_loop` in `src/simlin-engine/src/ltm_augment.rs` -- add a `target_element: Option<&str>` parameter (or a sibling fn) so the loop-score equation can select the right A2A slot.
- `generate_loop_score_equation` in `ltm_augment.rs` -- when a loop edge's resolved link score is dimensioned and `Link.to` carries an element subscript, emit `"…→to"[elem]`; otherwise emit the bare name as today.
- Element-stock collection for `partition_for_loop` (the existing element-level-stocks logic in the cross-element branch) is retained.

**Dependencies:** Phase 1 (link scores into arrayed targets must be meaningful before the loop product means anything).

**Done when:** Tests covering `ltm-503-cross-element-agg.AC2.1`-`AC2.5` pass (the `cross_element_ltm` fixture's `population[nyc]→migration_pressure[boston]→migration_in[nyc]→population[nyc]` loop and its symmetric twin are enumerated with subscripted link-score references; the loop-score series matches a hand calc at >=1 step; pure-A2A loops and pure-scalar models are unchanged); `test_cross_element_ltm_exhaustive`'s loose "at least one slot non-zero" assertions are tightened where the fixture dynamics warrant; `cargo test --workspace` is green.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Discovery-side alignment

**Goal:** Discovery (strongest-path) finds the cross-element loops, including scalar-source-in-a-loop loops that are silently undiscoverable today.

**Components:**
- Scalar->arrayed link-score naming: emit `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]` per target element for scalar-source -> arrayed-target edges (mirroring the arrayed->scalar `{from}[{elem}]→{to}` convention). Touches `link_score_dimensions` / the scalar->arrayed emission in `db_ltm.rs` and `link_score_var_name` in `ltm_augment.rs`.
- `parse_link_offsets` / `expand_a2a_link_offsets` in `src/simlin-engine/src/ltm_finding.rs` -- the Bare-A2A expander subscripts both sides only when both are arrayed; the new scalar->arrayed `{from}→{to}[{elem}]` names parse to `(from, to[elem])` via the existing `[`-in-`to` branch.
- `generate_loop_score_equation` (exhaustive path) -- for a scalar->arrayed loop edge, reference `"$⁚ltm⁚link_score⁚{from}→{to}[{elem}]"` (already a scalar variable per element) rather than subscripting an A2A variable.

**Dependencies:** Phase 2 (the loop-score reference scheme).

**Done when:** Tests covering `ltm-503-cross-element-agg.AC3.1`-`AC3.3` pass (`discover_loops_element_level` on `cross_element_ltm` finds the FixedIndex cross-element loop(s); discovery on a `total_pop = SUM(pop[*])`-style model finds `pop[*]→total_pop→migration[r]→pop[r]`; a `parse_link_offsets` unit test pins the scalar->arrayed naming); `test_cross_element_ltm_discovery` is tightened from "some subscripted loop" to the specific loop; `cargo test --workspace` is green.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Arrayed-result reducer support in the reducer link-score machinery

**Goal:** The reducer link-score machinery handles partial reduces (`SUM(m[D1,*])` -> arrayed result over `D1`), a prerequisite for hoisting every reducer.

**Components:**
- `classify_reducer` / `ReducerKind` in `src/simlin-engine/src/ltm_augment.rs` -- recognize a reduce over a strict subset of the source's axes (result is arrayed over the remaining axes), not just a full reduce to scalar.
- `try_cross_dimensional_link_scores` / `generate_element_to_scalar_equation` (a generalized sibling, `generate_element_to_reduced_equation`, may be cleaner) -- emit per-source-element link scores for `m[d1,d2] -> agg[d1]` edges where the link score is dimensioned over the result axes (`D1`) and parameterized by the reduced-axis source element (`d2`).
- `link_score_dimensions` in `db_ltm.rs` -- return the result-axis dims for an arrayed-result reducer target.
- `parse_link_offsets` -- confirm the per-element naming for arrayed-result reducer link scores parses correctly into the right element edges.

**Dependencies:** Phases 1-3 (uses the arrayed-equation `LtmSyntheticVar` from Phase 1 and the discovery naming conventions from Phase 3).

**Done when:** Tests covering `ltm-503-cross-element-agg.AC4.6` pass (a `SUM(m[D1,*])` reducer's link scores are computed per `(d1,d2)` and a cross-element loop over the reduced axis is scored from them); existing full-reduce (`SUM(pop[*])` -> scalar) behavior is unchanged; `cargo test --workspace` is green.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Reducer hoist (aggregate nodes)

**Goal:** Inlined array reducers that participate in feedback are treated as aggregate nodes; cross-element coupling through them composes by the chain rule; the diagonal conflation is resolved by construction.

**Components:**
- `enumerate_agg_nodes(model) -> Map<canonical_reducer_subexpr, agg_ident>` -- a shared, deterministic helper (location: `db_analysis.rs` or a small new module, callable from both the element-graph and LTM-variable salsa functions). Identifies each maximal reducer subexpression in LTM-relevant equations (including per-element exprs of `Ast::Arrayed` targets); a variable whose entire dt-equation is one reducer call maps to itself (no synthetic). Canonicalization via parsed-AST equality.
- `model_element_causal_edges` in `src/simlin-engine/src/db_analysis.rs` -- the reference-site walker reroutes a wildcard/dynamic reducer reference through the agg node: `pop[d] -> agg` (+ per-element for arrayed-result aggs) and `agg -> target[e]` instead of all-pairs `pop[d] -> target[e]`.
- `model_ltm_variables` in `src/simlin-engine/src/db_ltm.rs` -- emit `$⁚ltm⁚agg⁚{n}` auxiliaries (equation = the reducer subexpression, lowered normally; depends on the reducer's source(s)); emit `pop[d] -> agg` link scores via the Phase-4 machinery and `agg -> target` link scores via the Bare scalar->arrayed / A2A path; the partial-equation builder textually substitutes a recognized reducer subexpression with its agg name. Model equations are *not* rewritten.
- Loop reporting: `build_element_level_loops` / `build_loops_from_tiered` trim agg nodes from the reported `Loop` (collapse `pop[d] -> agg -> target[e]` to `pop[d] -> target[e]`; drop loops entirely internal to one agg). The loop-score *equation* is built on the un-trimmed path (so it includes the `pop[d]->agg` and `agg->target` link-score factors).
- Cleanup deferred to Phase 6: the `Wildcard`/`DynamicIndex` link-score path is now dead.

**Dependencies:** Phase 4 (arrayed-result reducer support); Phases 1-3.

**Done when:** Tests covering `ltm-503-cross-element-agg.AC4.1`-`AC4.5`, `AC4.7` pass (`share[r] = pop[r]/SUM(pop[*])` with feedback mints `$⁚ltm⁚agg⁚0` and the rerouted edges; the cross-element loop traverses agg twice and the reported loop has it trimmed; whole-RHS reducers mint no synthetic; nested reducers mint two aggs and identical subexprs dedupe; the heterogeneous-magnitude fixture's relative loop scores match a hand calc and differ from the lumped approximation; discovery finds the cross-element-through-agg loop); `cargo test --workspace` is green.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Cleanup, docs, and tracking

**Goal:** Remove the obviated Wildcard/Dynamic link-score path; update documentation and issue tracking.

**Components:**
- Remove `LINK_SCORE_WILDCARD_SUFFIX` / `LINK_SCORE_DYNAMIC_SUFFIX` usage from `link_score_var_name`, the suffix stripping (`strip_to_shape_suffix_with_rank`, `ShapeRank::Wildcard`/`DynamicIndex`) from `parse_link_offsets`, and the `Wildcard`/`DynamicIndex` cases from `resolve_link_score_name_for_loop`; remove the Wildcard/DynamicIndex TODO branch from `shape_aware_source_ref` (`ltm_augment.rs`). Decide whether `RefShape::Wildcard`/`DynamicIndex` survive as internal walker markers.
- Remove or rewrite tests pinned to the old behavior (e.g. `test_parse_link_offsets_wildcard_suffix_scalar`).
- `docs/design/ltm--loops-that-matter.md` -- document the aggregate-node treatment and element-level cross-element loop scoring.
- `src/simlin-engine/CLAUDE.md` -- new `$⁚ltm⁚agg⁚{n}` synthetic family; retired Wildcard path; `LtmSyntheticVar.equation` type change.
- `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` measurement postscript -- note the SCC/loop-count shift on reducer-bearing fixtures (re-measure `cross_element_ltm` and any reducer-bearing fixture; WRLD3 is scalar so unaffected).
- GH #503 closed referencing the implementing commit(s); epic #488's checklist ticked.

**Dependencies:** Phase 5.

**Done when:** Tests covering `ltm-503-cross-element-agg.AC5.1`-`AC5.2`, `AC6.1`-`AC6.2`, `AC7.1`-`AC7.2` pass / are satisfied (no `⁚wildcard`/`⁚dynamic` variables emitted; scalar and pure-A2A golden data unchanged; `cargo test --workspace` within the 3-minute cap; docs and issues updated); the pre-commit hook passes.
<!-- END_PHASE_6 -->

## Additional Considerations

**Error handling / degenerate inputs.** The agg node's equation is the reducer subexpression parsed normally; if it fails to parse or lower (it shouldn't -- it was a subexpression of an already-valid equation), the existing LTM error path applies (the link/loop scores referencing it get the fragment compiler's stub-dep fallback, i.e. a zero contribution -- the same graceful degradation already used when a link score variable is missing). A reducer over a scalar source (degenerate; the parser would normally reject `SUM(scalar)`) is not hoisted -- `enumerate_agg_nodes` only fires for reducers over arrayed sources.

**`PREVIOUS`/`INIT` snapshots.** `$⁚ltm⁚agg⁚n` is an ordinary auxiliary with no init equation -- it is a pure function of its source(s), so the existing dependency sort places it before its consumers and the existing `prev_values` snapshot makes `PREVIOUS(agg)` available. No new machinery; verify in Phase 5 tests.

**Layout / per-element allocation.** A link score whose equation is `Equation::Arrayed` (Phase 1) must be allocated per-element by the layout/results allocator the same way an `Equation::ApplyToAll` arrayed var is. `parse_link_offsets`'s `expand_*` helpers are layout-driven (base offset + element index) and so are independent of which arrayed-equation variant produced the var; verify in Phase 1/3 tests.

**Edge: per-element-equation target with a different reducer per element** (`x[a] = SUM(p[*]); x[b] = MEAN(p[*])`). `enumerate_agg_nodes` walks per-element expressions, so each gets its own agg node. In scope; the extra fiddliness is that the walker iterates `Ast::Arrayed`'s map rather than a single expr.

**Interaction with #487 (A2A `partition = None`).** Independent -- #487 is a `partition_for_loop` keying bug for pure-A2A loops; this work touches the cross-element/mixed branches and the element-stock collection there is retained. The new agg-bearing loops carry element-level stocks (as the cross-element branch already does), so they resolve their partitions. If a new fixture here happens to also exercise #487, note it but do not fold the fix in.

## Risks and Open Questions

**R1: Golden-data drift.** Reducer-bearing and cross-element fixtures (`cross_element_ltm`, `arrayed_population_ltm` if it uses a reducer in a loop, any new fixtures) will see loop-count / score-magnitude changes -- the cross-element loops go from garbage to correct, and reducer hoisting adds agg nodes. WRLD3 and other scalar models are unaffected (no reducers, no per-element-eqn targets). Plan: run `simulate_ltm` with no golden updates first, investigate every diff, document per-test reasoning.

**R2: `cross_element_ltm` already has relaxed assertions.** `docs/tech-debt.md` notes `test_cross_element_ltm_exhaustive`'s assertions were relaxed to "at least one slot non-zero" because the broadcast/`"0"`-partial bugs hid reality. After Phases 1-2 those slots carry meaningful values; tighten the assertions where the fixture dynamics support it, document where they don't (the `MAX(...)` semantics in `migration_in`/`migration_out` legitimately zero some slots).

**R3: Scope boundary -- arrayed-result reducers.** Phase 4 commits to supporting partial reduces (`SUM(m[D1,*])`). If the generalization of `generate_element_to_scalar_equation` proves larger than expected, Phase 4 may itself need splitting; flag at implementation-planning time. (Decided in design review: support it; do not fall back to all-pairs.)

**R4: Loop-reporting trim is a new operation.** Trimming agg nodes from reported loops while keeping them in the loop-score equation is analogous to the module-pathway trimming but is new code in `build_element_level_loops`/`build_loops_from_tiered`. Risk is mostly in keeping the trimmed `Loop`'s `links`/`stocks`/`polarity` consistent with what downstream consumers (`partition_for_loop`, layout metadata, JSON SDAI) expect. Covered by AC2.x / AC4.2 tests.

**R5: Does the loop-score equation's textual reducer-substitution always find the subexpression?** The partial-equation builder operates on the *target's* equation text; the agg's canonical form was derived from the same AST, so substitution by AST-subtree match is reliable. Edge: if a target references the same reducer subexpression both inside and outside a `PREVIOUS()` already (unlikely), the substitution must not double-wrap. Covered by Phase 5 tests.

## Out of Scope

- **#487** (A2A `partition = None`): independent `partition_for_loop` keying bug; fix separately.
- **#483** (STDDEV/RANK ceteris-paribus partials fall back to delta-ratio): the per-element reducer partials for STDDEV/RANK are a known approximation in `generate_element_to_scalar_equation`; Phase 4 extends that machinery to arrayed results but does *not* change the STDDEV/RANK approximation. If hoisting surfaces it more prominently, note it on #483.
- **#309** (discovery iterates at `save_step` granularity, not `dt`): unrelated; this work does not change discovery's time-stepping.
- General refactors of `build_element_level_loops` beyond what (B)/(D) require.
