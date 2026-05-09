# LTM Cross-Element Aggregate Scoring — Phase 3: Discovery-side alignment

**Goal:** Discovery mode (the "strongest-path" loop-finding mode used for large models) finds the cross-element loops, including scalar-source-in-a-loop loops that are silently undiscoverable today, because the per-timestep `SearchGraph` it builds (by parsing LTM link-score variable names) now traverses the correct element graph.

**Architecture:** The one real gap on the discovery side: a scalar-source → arrayed-target link score (`total_pop → migration` where `total_pop` is scalar, `migration[Region]` is arrayed) is emitted today as a Bare A2A link score with `dimensions = ["Region"]`; when `parse_link_offsets`'s `expand_a2a_link_offsets` expands it, it subscripts *both* sides, inventing a `total_pop[nyc]` node that doesn't match the unsubscripted `total_pop` node from the `pop[d]→total_pop` edges — so the loop is unreachable in the search graph. Fix: emit scalar→arrayed link scores under a per-target-element name `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]`, one scalar `LtmSyntheticVar` per target element (mirroring the existing arrayed→scalar `{from}[{elem}]→{to}` convention emitted by `try_cross_dimensional_link_scores`). Then `parse_link_offsets`'s existing `[`-in-`to` single-passthrough branch parses `{from}→{to}[{elem}]` to the edge `(from, to[elem])` with no parser change; `generate_loop_score_equation` (exhaustive path) references that per-element scalar variable directly. The FixedIndex cross-element direction (`pop[nyc]→mp` over a dimension) already expands correctly via `expand_fixed_from_a2a_link_offsets` — no change needed there.

**Tech Stack:** Rust; `simlin-engine`; `ltm_finding.rs` (the discovery-mode strongest-path machinery: `parse_link_offsets`, `SearchGraph`, `discover_loops_with_graph`); `db_ltm.rs` (`link_score_dimensions`, `try_cross_dimensional_link_scores`, `model_ltm_variables`); `ltm_augment.rs` (`generate_loop_score_equation`, `resolve_link_score_name_for_loop`).

**Scope:** Phase 3 of 6 from `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md`.

**Codebase verified:** 2026-05-09 (codebase-investigator).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### ltm-503-cross-element-agg.AC3: Discovery mode finds cross-element loops
- **ltm-503-cross-element-agg.AC3.1 Success:** `discover_loops_element_level` on the `cross_element_ltm` fixture finds a loop whose links include `population[nyc] → migration_pressure[boston]` (or the symmetric `population[boston] → migration_pressure[nyc]`), not merely "some subscripted loop".
- **ltm-503-cross-element-agg.AC3.2 Success:** On a model that factors out a scalar reducer (`total_pop = SUM(pop[*])`, `migration[r] = total_pop * c - pop[r] * c`, `pop[r]` stock fed by `migration[r]`), discovery finds the loop `pop[*] → total_pop → migration[r] → pop[r]` -- i.e. the scalar->arrayed link score's element slots resolve to `(total_pop, migration[d])` edges, not `(total_pop[d], migration[d])`. (Pre-fix this loop was silently undiscoverable.)
- **ltm-503-cross-element-agg.AC3.3 Success:** A `parse_link_offsets` unit test: a scalar->arrayed link score named `$⁚ltm⁚link_score⁚total_pop→migration[nyc]` resolves to the edge `(total_pop, migration[nyc])`.

> Phase 3 depends on Phase 2 (the loop-score reference scheme — `resolve_link_score_name_for_loop`'s `target_element` parameter and `generate_loop_score_equation`'s subscripted references).

---

## Context for the implementer (read before starting)

### How discovery mode works

```
analysis.rs run_ltm_pipeline (analysis.rs:114-185) — production:
   compile + simulate LTM model -> Results
   build element-level CausalGraph: model_element_causal_edges(...) -> causal_graph_from_element_edges(...)
   ltm_vars = model_ltm_variables(db, model_name, project);  dm_dims = project_datamodel_dims(db, project);
   discover_loops_with_graph(&results, &causal_graph, &stocks, &ltm_vars.vars, dm_dims)
        |
discover_loops_with_graph (ltm_finding.rs:681-827):
   parse_link_offsets(results, ltm_vars, dims) -> Vec<LinkOffset {from, to, offset}>
   -> link_offset_map
   per simulation step: SearchGraph::from_results(results, step, &link_offset_map, &causal_graph)  [SearchGraph: ltm_finding.rs:60-66]
   -> find_strongest_loops (ltm_finding.rs:138-170) — per-stock DFS, strongest cycle through each stock
   -> assemble FoundLoop with per-step scores
```

`parse_link_offsets` (`ltm_finding.rs:318-459`) — four-way dispatch (constants: `LINK_SCORE_PREFIX = "$⁚ltm⁚link_score⁚"` at `ltm_finding.rs:44`, `LTM_LINK_SEP = '→'` at `ltm_finding.rs:47`):
- Branch 1 — `from_str.contains('[') && !var_dims.is_empty()` ⇒ `expand_fixed_from_a2a_link_offsets` (FixedIndex source on an A2A var; keeps `from` pinned, subscripts `to` per element of `var_dims`). **Already correct for FixedIndex cross-element edges (`pop[nyc]→mp` over `Region`) — no change.**
- Branch 2 — `from_str.contains('[') || to_str.contains('[')` ⇒ single passthrough `((Ident::new(from_str), Ident::new(to_str)), offset)`. **The new `{from}→{to}[{elem}]` names land here** (the `[elem]` survives `strip_to_shape_suffix_with_rank`, which only strips `⁚wildcard`/`⁚dynamic`). Also handles the existing arrayed→scalar `{from}[{elem}]→{to}` scalar reducer names (their `var_dims` is empty).
- Branch 3 — `var_dims.is_empty()` ⇒ single scalar passthrough.
- Branch 4 — else (Bare A2A) ⇒ `expand_a2a_link_offsets` (subscripts **both** sides identically over `var_dims`). **This is the broken path for scalar→arrayed link scores today** — fix is to stop *naming* those edges as Bare-A2A so they never reach Branch 4.
- Then: dedupe by `(from, to)` keeping lowest `(ShapeRank, offset)` (`ShapeRank` at `ltm_finding.rs:310-316`: `Bare=0, FixedIndex=1, Wildcard=2, DynamicIndex=3`), then sort.

### Verified code locations (from codebase-investigator, 2026-05-09)

| Symbol | Location | Notes |
|---|---|---|
| `parse_link_offsets` | `src/simlin-engine/src/ltm_finding.rs:318-459` | reads only `.name` and `.dimensions` of `LtmSyntheticVar` — never `.equation` |
| `expand_a2a_link_offsets` | `src/simlin-engine/src/ltm_finding.rs:486-510` | subscripts **both** `from` and `to` over `var_dims`; only has `var_dims` (the link-score var's dims), no access to the source var's actual shape — hence the `total_pop[nyc]` invention |
| `expand_fixed_from_a2a_link_offsets` | `src/simlin-engine/src/ltm_finding.rs:522-547` | keeps `from_with_index` literal, subscripts only `to` — **already correct for FixedIndex cross-element** |
| `strip_to_shape_suffix_with_rank` | `src/simlin-engine/src/ltm_finding.rs:467-475` | strips `LINK_SCORE_WILDCARD_SUFFIX`/`LINK_SCORE_DYNAMIC_SUFFIX`, returns `ShapeRank`. Does NOT touch `[elem]` subscripts. |
| `resolve_dim_element_tuples` | `src/simlin-engine/src/ltm_finding.rs:553-587` | row-major cartesian product of named/indexed dims; `None` if unresolvable |
| `discover_loops_with_graph` | `src/simlin-engine/src/ltm_finding.rs:681-827` | takes `ltm_vars: &[LtmSyntheticVar]` + `dims` and threads them into `parse_link_offsets` — **production already passes `model_ltm_variables(...).vars` here**, so once the right vars are emitted, discovery picks them up with no plumbing change |
| `discover_loops` (non-element, all-scalar) | `src/simlin-engine/src/ltm_finding.rs:660-669` | builds a non-element `CausalGraph`, calls `discover_loops_with_graph(..., &[], &[])` — **empty ltm_vars/dims**, every link treated as scalar. Used by the test helper `discover_loops_from_path`. **Not suitable for cross-element assertions.** |
| `SearchGraph` | `src/simlin-engine/src/ltm_finding.rs:60-66` | `{ adj: HashMap<Ident<Canonical>, Vec<ScoredEdge>>, stocks: Vec<Ident<Canonical>> }`; `from_results` at `:114-130`; `find_strongest_loops` at `:138-170`; `check_outbound_uses` at `:186-238` |
| `link_score_dimensions` (nested in `model_ltm_variables`) | `src/simlin-engine/src/db_ltm.rs:2654-2738` | returns the target's dims when `dims_compatible` (which includes `from_dims.is_empty()` — i.e. **scalar source → arrayed target currently gets the target's dims** ⇒ Bare-A2A name ⇒ Branch 4 mis-expansion). This is the spot to change: detect `from_dims.is_empty() && !to_dims.is_empty()` and route to per-target-element scalar emission instead. |
| `try_cross_dimensional_link_scores` (nested) | `src/simlin-engine/src/db_ltm.rs:2753-2825` | the **template** for the per-element scalar emission: fires for (arrayed source, scalar target) reducers, emits `LtmSyntheticVar { name: format!("$⁚ltm⁚link_score⁚{}[{}]→{}", from, element, to), equation: generate_element_to_scalar_equation(...), dimensions: vec![] }` per source element; constructor at `:2818`. Phase 3's scalar→arrayed sibling does the mirror: name `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]` per *target* element, `dimensions: vec![]`. |
| `emit_per_shape_link_scores` / `enumerate_shapes` (nested) | `src/simlin-engine/src/db_ltm.rs:2882-2916 / 2838-2867` | for each shape, `link_score_equation_text_shaped(...)`, then `lsv.name = link_score_var_name(from, to, &shape)`, `lsv.dimensions = link_score_dimensions(...)` |
| main link-score loop in `model_ltm_variables` | `src/simlin-engine/src/db_ltm.rs:2918-2987` | discovery/sub-model models iterate **all** causal edges; exhaustive iterate detected loops' links (`strip_subscript`). Per edge: try `try_cross_dimensional_link_scores` first (`continue` if `Some`), else `emit_per_shape_link_scores`. **Phase 3's scalar→arrayed branch slots in here, before/alongside `try_cross_dimensional_link_scores`.** |
| `link_score_var_name` | `src/simlin-engine/src/ltm_augment.rs:458` | `FixedIndex(elems)` ⇒ `{from}[{elems.join(",")}]→{to}`; Wildcard/DynamicIndex ⇒ `{to}⁚wildcard`/`{to}⁚dynamic`; Bare ⇒ `{from}→{to}`. **Cannot produce `{from}→{to}[{elem}]`** — mirror `try_cross_dimensional_link_scores`'s direct `format!` instead of extending this. |
| `generate_loop_score_equation` / `resolve_link_score_name_for_loop` / `find_fixed_index_emitted_name` | `src/simlin-engine/src/ltm_augment.rs:965 / 904 / 930` | Phase 2 added a `target_element` parameter / `(name, subscript)` return; Phase 3 extends the resolution to recognize a `{from}→{to}[{elem}]`-shaped emitted name (a per-target-element scalar var) and emit it as the bare quoted name `"$⁚ltm⁚link_score⁚{from}→{to}[{elem}]"` (no extra `[...]` subscript — the element is already in the name), **and not strip the `[elem]` off the `to` side**. |
| `test_mixed_loop_scalar_per_element_scores` | `src/simlin-engine/tests/simulate_ltm.rs:3486-3551` | `TestProject` model: `population[Region={NYC,Boston}]` stock (inflows `births`, `migration`), `birth_rate[Region]=0.05`, `births[Region]=population*birth_rate`, `total_pop`(scalar)`=SUM(population[*])`, `migration[Region]=total_pop*0.01 - population*0.01`; `.compile_ltm_incremental_with_partitions()`. **Exhaustive mode only — does NOT exercise discovery.** AC3.2 needs a NEW sibling discovery test on the same model. |
| `discover_loops_element_level` (test helper) | `src/simlin-engine/tests/simulate_ltm.rs:3570-3610` | mirrors `run_ltm_pipeline`: enables LTM + discovery, compiles, simulates, builds element-level causal graph, fetches `model_ltm_variables` + datamodel dims, calls `discover_loops_with_graph(&results, &causal_graph, &stocks, &ltm_vars.vars, dm_dims)`. **This is the helper AC3.1 and AC3.2 tests should use.** Takes a `&datamodel::Project`. |
| `discover_loops_from_path` (test helper) | `src/simlin-engine/tests/simulate_ltm.rs:296-311` | uses `ltm_finding::discover_loops` (non-element, all-scalar). Not for cross-element. |
| `test_cross_element_ltm_discovery` | `src/simlin-engine/tests/simulate_ltm.rs:4669-4711` | loads `test/cross_element_ltm/cross_element.stmx`, `found = discover_loops_element_level(&datamodel_project)`; loose assertions: `!found.is_empty()`, some loop has a `[`-subscripted `from`, every loop has ≥1 link. **AC3.1 tightens this** to assert the specific cross-element loop `population[nyc]→migration_pressure[boston]` (or symmetric `population[boston]→migration_pressure[nyc]`) is among `found`. |
| `parse_link_offsets` unit tests | `src/simlin-engine/src/ltm_finding.rs` `mod tests` (starts ~`:968`) | `test_parse_link_offsets` (`:1323`, scalar), `test_parse_link_offsets_a2a_expansion` (`:1363`, A2A var dims ⇒ subscript both sides), `test_parse_link_offsets_cross_dim_passthrough` (`:1447`), `test_parse_link_offsets_fixed_index_from_a2a_expansion` (`:1616`), `test_parse_link_offsets_fixed_index_from_scalar` (`:1681` — name `$⁚ltm⁚link_score⁚src[nyc]→dst`, no matching `ltm_vars` ⇒ single passthrough `src[nyc]→dst` with FixedIndex rank — **the exact pattern for AC3.3, with the subscript flipped to the `to` side**), `test_parse_link_offsets_wildcard_suffix_scalar` (`:1512`), dedup tests at `:1729 / :1784 / :1856`. Helper `make_results_with_offsets(offsets: &[(&str, usize)], step_size: f64) -> Results` at `~ltm_finding.rs:1485-1510`. |
| `test_model_ltm_variables_scalar_to_arrayed_link_score` | `src/simlin-engine/src/db_ltm_unified_tests.rs:289-325` | currently asserts the scalar→arrayed link score (`growth_factor→births`) has `dimensions = ["Region"]`. **Phase 3 changes this**: it should now assert one scalar `LtmSyntheticVar` per target element named `$⁚ltm⁚link_score⁚growth_factor→births[nyc]` etc. with `dimensions = vec![]`. Update this test. |
| `cross_dim_link_score_equations_match_between_exhaustive_and_discovery` | `src/simlin-engine/src/db_ltm_unified_tests.rs:1452-1527` | `total_pop = SUM(pop[*])`; checks `pop[nyc]→total_pop`/`pop[boston]→total_pop` equations match between modes; defends against the `sum(PREVIOUS(pop[*]))` zero-numerator bug. **Should keep passing.** |
| `cross_element_ltm` fixture | `test/cross_element_ltm/cross_element.stmx` | (as in phases 1-2; includes `total_population = SUM(population[*])`, a scalar reducer, and the per-element migration vars) |

> **Note on Phase-1 ordering:** Phase 1 changed `LtmSyntheticVar.equation` from `String` to `datamodel::Equation`. Any `parse_link_offsets` unit test that constructs an `LtmSyntheticVar` literally must use `datamodel::Equation::Scalar(...)` (or an empty equation helper) rather than `String::new()`. (Pre-Phase-1 examples in `ltm_finding.rs` used `equation: String::new()`.)

### Conventions

TDD mandatory. `parse_link_offsets` unit tests: `ltm_finding.rs` `mod tests`, Style D (build a `Results` with `make_results_with_offsets`, assert on `parse_link_offsets` output). `model_ltm_variables` link-score-naming assertions: `db_ltm_unified_tests.rs` (`TestProject` ⇒ `sync_from_datamodel` ⇒ `model_ltm_variables`, inspect `.vars`). Discovery end-to-end: `simulate_ltm.rs` via `discover_loops_element_level`. Each new unit test under ~2s; `cargo test --workspace` under the 3-minute cap. Commits: `engine: ...`, no emoji, no `Co-Authored-By`, never `--no-verify`.

---

## Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Emit scalar→arrayed link scores as per-target-element scalar variables

**Verifies:** ltm-503-cross-element-agg.AC3.2 (partial — emission side), ltm-503-cross-element-agg.AC3.3

**Files:**
- Modify: `src/simlin-engine/src/db_ltm.rs` — `link_score_dimensions` (`:2654-2738`) and the main link-score emission loop / `emit_per_shape_link_scores` callsite (`:2882-2916`, `:2918-2987`). Add a `try_scalar_to_arrayed_link_scores` sibling helper (mirroring `try_cross_dimensional_link_scores` at `:2753-2825`) that, when `from` is scalar and `to` is arrayed and the edge is real, emits one `LtmSyntheticVar` per target element: `name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]", from, to, element)`, `dimensions: vec![]`, `equation = <the per-target-element link-score equation>`.
- Modify: `src/simlin-engine/src/ltm_augment.rs` — possibly add a helper `generate_scalar_to_element_equation` (or reuse `generate_auxiliary_to_auxiliary_equation` / `generate_stock_to_flow_equation` with `RefShape::Bare` source and the target element pinned). The per-target-element equation for `total_pop → migration[nyc]` is: the partial of `migration[NYC]`'s equation w.r.t. `total_pop` live, everything else PREVIOUS, wrapped in the standard link-score guard form with `to_q = "migration[nyc]"` and `from_source_q = "total_pop"`. If `migration` is A2A (`Equation::ApplyToAll`), the per-element equation is the same formula for every element with the element pinned on the `to` side; if `migration` is `Ast::Arrayed`, it's that element's own expression's partial (this overlaps Phase 1's machinery — reuse `build_partial_equation_shaped` on the right per-element text).
- Test: `src/simlin-engine/src/ltm_finding.rs` `mod tests` (AC3.3), `src/simlin-engine/src/db_ltm_unified_tests.rs` (emission naming), and update `test_model_ltm_variables_scalar_to_arrayed_link_score` (`db_ltm_unified_tests.rs:289-325`).

**Implementation contract:**
- A scalar-source → arrayed-target link score is emitted as **one scalar `LtmSyntheticVar` per target element**, named `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]`, `dimensions: vec![]`. It is **not** emitted as a single Bare-A2A var with `dimensions = [target_dims]`.
- `link_score_dimensions`: when `from_dims.is_empty() && !to_dims.is_empty()` (scalar source, arrayed target), do **not** return the target's dims; instead this edge is handled by the new per-target-element emitter (return `vec![]` and have the caller route to `try_scalar_to_arrayed_link_scores`). (Verify exactly where `link_score_dimensions`'s result is consumed — `emit_per_shape_link_scores` stamps `lsv.dimensions` and the main loop uses it to decide A2A-vs-not — so the cleanest is: in the main link-score emission loop, try `try_scalar_to_arrayed_link_scores` (when source scalar & target arrayed) before `emit_per_shape_link_scores`, and `continue` if it returns `Some`, exactly like `try_cross_dimensional_link_scores`.)
- The `RefShape::Wildcard`/`RefShape::DynamicIndex` paths and `link_score_var_name`'s suffix logic are untouched in Phase 3 (Phase 6 removes them).
- This is a *naming + structure* change; the per-element equations carry the same information that the Bare-A2A var's `Equation::ApplyToAll`/`Equation::Arrayed` carried after Phase 1 — just spread across N scalar vars. Existing simulations of scalar→arrayed-bearing models should produce the same per-element values (now stored in N scalar slots instead of one A2A slot).

**Testing:** TDD. Tests must verify:
- AC3.3: a `parse_link_offsets` unit test (`ltm_finding.rs` `mod tests`, following `test_parse_link_offsets_fixed_index_from_scalar` at `:1681`): `offsets = [("$⁚ltm⁚link_score⁚total_pop→migration[nyc]", 0usize)]`; `results = make_results_with_offsets(&offsets, 1.0)`; `parsed = parse_link_offsets(&results, &[], &[])` (no `ltm_vars` entry needed — `var_dims` empty ⇒ Branch 2 single passthrough); assert `parsed.len() == 1`, `parsed[0].from.as_str() == "total_pop"`, `parsed[0].to.as_str() == "migration[nyc]"`.
- Emission naming: a `db_ltm_unified_tests.rs` test (or update `test_model_ltm_variables_scalar_to_arrayed_link_score`) — build `TestProject` with a scalar var feeding an arrayed var (e.g. `growth_factor` scalar, `births[Region] = population * growth_factor`), enable LTM, fetch `model_ltm_variables`, assert it emits `$⁚ltm⁚link_score⁚growth_factor→births[nyc]`, `$⁚ltm⁚link_score⁚growth_factor→births[boston]`, ... each with `dimensions = vec![]`, and does NOT emit `$⁚ltm⁚link_score⁚growth_factor→births` with `dimensions = ["Region"]`.
- `cross_dim_link_score_equations_match_between_exhaustive_and_discovery` (`db_ltm_unified_tests.rs:1452`) still passes.

**Verification:**
- Run: `cargo test -p simlin-engine` — green; the AC3.3 unit test, the emission-naming test, and the updated `test_model_ltm_variables_scalar_to_arrayed_link_score` pass.
- Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm` — green (existing scalar→arrayed-bearing model tests still produce the right values, now in scalar slots).

**Commit:** `engine: emit scalar-to-arrayed link scores per target element`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Reference per-target-element scalar link scores in loop-score equations (exhaustive path)

**Verifies:** ltm-503-cross-element-agg.AC3.2 (the exhaustive loop-score side; AC3.2's discovery side is Task 3)

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` — `resolve_link_score_name_for_loop` (`:904`) / `generate_loop_score_equation` (`:965`): when a loop edge is scalar-source → arrayed-target visited at element `e`, resolve the emitted name `$⁚ltm⁚link_score⁚{from}→{to}[{e}]` (it's a scalar variable per element) and emit it as the bare quoted name `"$⁚ltm⁚link_score⁚{from}→{to}[{e}]"` (no extra `[...]` subscript — the element is already in the name). Do **not** strip the `[e]` off the loop edge's `to` for this case. This is the natural extension of Phase 2's `target_element` work — Phase 2 handled A2A link scores (subscript-after-quote); Phase 3 adds the scalar-per-element-name case (element-in-name).
- Modify (only if needed): `src/simlin-engine/src/db_ltm.rs` — the loop construction in `build_element_level_loops` (Phase 2) must keep the `to` subscript on a scalar→arrayed loop edge (e.g. `total_pop → migration[nyc]`) so `generate_loop_score_equation` knows to look up `$⁚ltm⁚link_score⁚total_pop→migration[nyc]`. Phase 2's "keep `to[e]` when the link score is dimensioned" rule needs the corollary "keep `to[e]` when the link score is a per-target-element scalar var" — i.e. keep `to[e]` whenever the target is arrayed and the source is scalar. Verify Phase 2 left a clean hook for this.
- Test: `src/simlin-engine/src/db_ltm_unified_tests.rs` (loop-score-equation assertion), `src/simlin-engine/tests/simulate_ltm.rs` (end-to-end on the `test_mixed_loop_scalar_per_element_scores` model — but exhaustive mode — or extend the AC2.2-style hand-calc check).

**Implementation contract:**
- For a loop edge `scalar_source → arrayed_target[e]`, the loop-score equation term is `"$⁚ltm⁚link_score⁚{scalar_source}→{arrayed_target}[{e}]"` (a per-element scalar variable, referenced bare). For a loop edge `arrayed_source[d] → scalar_target` (the reducer direction, already emitted by `try_cross_dimensional_link_scores` as `$⁚ltm⁚link_score⁚{arrayed_source}[{d}]→{scalar_target}`), the term is `"$⁚ltm⁚link_score⁚{arrayed_source}[{d}]→{scalar_target}"` — `find_fixed_index_emitted_name` (with Phase 2's `target_element`-aware matching) already resolves this.
- No A2A-subscript-after-quote here — these are all scalar vars.

**Testing:** TDD.
- Build the `test_mixed_loop_scalar_per_element_scores`-style model (`total_pop = SUM(population[*])`, `migration[Region] = total_pop*0.01 - population*0.01`, `population[Region]` stock fed by `migration`+`births`), compile in exhaustive mode, fetch `model_ltm_variables`, find the loop-score var for the loop `population[r] → migration[r] → total_population → population[r]` ... wait, the loop is `population[*] → total_pop → migration[r] → population[r]`. Its loop-score equation should be the product of: `"$⁚ltm⁚link_score⁚population[r]→total_pop"` (arrayed→scalar reducer link score, per source element), `"$⁚ltm⁚link_score⁚total_pop→migration[r]"` (the new scalar→arrayed per-target-element link score), `"$⁚ltm⁚link_score⁚migration→population"[r]` (flow→stock, A2A — subscripted-after-quote, Phase 2). Assert the equation text contains these references. (The exact loop topology / which element `r` — figure out from `model_detected_loops`; there may be one loop per region.)
- Also: a hand-calc value check at >= 1 step (sibling to AC2.2).

**Verification:** `cargo test -p simlin-engine` — green. `cargo test -p simlin-engine --features file_io --test simulate_ltm` — green.

**Commit:** `engine: reference scalar-to-arrayed link scores in loop-score equations`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Discovery-mode integration tests — cross-element and scalar-reducer loops

**Verifies:** ltm-503-cross-element-agg.AC3.1, ltm-503-cross-element-agg.AC3.2 (the discovery side)

**Files:**
- Modify: `src/simlin-engine/tests/simulate_ltm.rs` — tighten `test_cross_element_ltm_discovery` (`:4669-4711`); add a new test `test_scalar_reducer_loop_discovery` (or similar) using `discover_loops_element_level` on the `test_mixed_loop_scalar_per_element_scores`-style model.
- Possibly modify: `src/simlin-engine/src/ltm_finding.rs` — `parse_link_offsets` only if a defensive guard is wanted (the design's Change 2 — "the Bare-A2A expander subscripts both sides only when both are arrayed"); strictly unnecessary if Task 1 stops emitting Bare-A2A names for scalar→arrayed edges, but a guard in `expand_a2a_link_offsets` is harmless. Decide during implementation; if added, add a unit test for it.

**Implementation contract:** No new production logic beyond Tasks 1-2 (and any optional defensive guard). This task is the discovery-mode end-to-end coverage.

**Testing:** TDD — write these tests, confirm they fail against `main` (the loops aren't found / the assertions are too loose), confirm Tasks 1-2 make them pass.
- AC3.1: tighten `test_cross_element_ltm_discovery` — `found = discover_loops_element_level(&cross_element_project)`; assert that **some loop in `found`** has a link `population[nyc] → migration_pressure[boston]` (or the symmetric `population[boston] → migration_pressure[nyc]`) — i.e. inspect `loop_.links` for a `Link` whose `from.as_str() == "population[nyc]"` and `to.as_str() == "migration_pressure[boston]"` (or check both directions). Keep the existing weaker assertions too (non-empty, every loop has ≥1 link).
- AC3.2: new test — build the scalar-reducer model (`total_pop = SUM(population[*])`, `migration[Region] = total_pop*0.01 - population*0.01`, `population[Region]` stock fed by `migration` + `births`), `found = discover_loops_element_level(&project)`; assert `found` contains the loop `population[*] → total_pop → migration[r] → population[r]` for some region `r` — i.e. a loop whose links include an edge `(total_pop, migration[nyc])` (NOT `(total_pop[nyc], migration[nyc])` — the scalar source must stay unsubscripted) and an edge `(population[nyc], total_pop)` (the reducer edge). Document in a comment that pre-fix this loop was undiscoverable because `expand_a2a_link_offsets` invented a `total_pop[nyc]` node.

**Verification:**
- Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm` — green; `test_cross_element_ltm_discovery` (tightened) and the new `test_scalar_reducer_loop_discovery` pass; `test_mixed_loop_scalar_per_element_scores` (exhaustive) still passes.
- Run: `cargo test --workspace` — green within the 3-minute cap.

**Commit:** `engine: cover cross-element and scalar-reducer loop discovery`
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase 3 done-when checklist

- [ ] Scalar→arrayed link scores are emitted as per-target-element scalar `LtmSyntheticVar`s named `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]`, `dimensions: vec![]` — no longer as a Bare-A2A var with `dimensions = [target_dims]`.
- [ ] `link_score_dimensions` / the main link-score loop route scalar-source→arrayed-target edges to the new per-target-element emitter.
- [ ] `generate_loop_score_equation` references those per-element scalar vars by their (element-in-name) bare quoted name in exhaustive loop-score equations.
- [ ] AC3.3 — the `parse_link_offsets` unit test pins `$⁚ltm⁚link_score⁚total_pop→migration[nyc]` ⇒ `(total_pop, migration[nyc])`.
- [ ] AC3.1 — `test_cross_element_ltm_discovery` tightened: discovery finds the cross-element `population[nyc]→migration_pressure[boston]` (or symmetric) loop.
- [ ] AC3.2 — new test: discovery finds `pop[*]→total_pop→migration[r]→pop[r]` (scalar source stays unsubscripted in the search graph).
- [ ] `test_model_ltm_variables_scalar_to_arrayed_link_score` updated to the new naming; `cross_dim_link_score_equations_match_between_exhaustive_and_discovery` still passes.
- [ ] `cargo test --workspace` green within the 3-minute cap; clippy/fmt clean (run `git commit` and let the pre-commit hook gate it).
