# LTM Cross-Element Aggregate Scoring — Phase 2: Element-level cross-element loops (exhaustive path)

**Goal:** A cross-element feedback loop (one that genuinely traverses different elements at different points, e.g. `population[nyc] → migration_pressure[boston] → migration_in[nyc] → population[nyc]`) is scored from the actual per-element link scores along its element-level path, instead of being collapsed to a scalar `Loop` whose loop-score equation references the *diagonal* apply-to-all link scores.

**Architecture:** `build_element_level_loops` in `db_ltm.rs` currently has an `is_cross_element` branch that collapses the element-level circuit to a variable-level (diagonal-named) `Loop`. We rewrite that branch to keep element subscripts on each `Link` (generalizing the existing "mixed/scalar" branch's per-link construction so it also retains the subscript on the `to` side). `generate_loop_score_equation` then emits subscripted link-score references — `"$⁚ltm⁚link_score⁚{from}→{to}"[e]` for an A2A (dimensioned) link score visited at element `e` — via a `target_element`-aware `resolve_link_score_name_for_loop`. The loop-score *variable* stays `Equation::Scalar` (a cross-element loop visits fixed elements; it is not parameterized by a free dimension). Pure-A2A loops and pure-scalar models are untouched.

**Tech Stack:** Rust; `simlin-engine`; salsa tracked functions; the `ltm` module's `Loop`/`Link`/`CausalGraph` types; the tiered loop-enumeration pipeline (`model_loop_circuits_tiered` → `build_loops_from_tiered` → `build_element_level_loops`).

**Scope:** Phase 2 of 6 from `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md`.

**Codebase verified:** 2026-05-09 (codebase-investigator).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### ltm-503-cross-element-agg.AC2: Cross-element loops scored element-level (exhaustive path)
- **ltm-503-cross-element-agg.AC2.1 Success:** In the `cross_element_ltm` fixture, the loop `population[nyc] → migration_pressure[boston] → migration_in[nyc] → population[nyc]` is enumerated, and its `loop_score` equation references `"$⁚ltm⁚link_score⁚population[nyc]→migration_pressure"[boston]`, `"$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in"[nyc]`, and `"$⁚ltm⁚link_score⁚migration_in→population"[nyc]` -- each subscripted at the element the loop visits -- not the unsubscripted A2A diagonal names.
- **ltm-503-cross-element-agg.AC2.2 Success:** That loop's `loop_score` series matches a hand calculation at >= 1 timestep (within 1e-6).
- **ltm-503-cross-element-agg.AC2.3 Success:** The symmetric loop `population[boston] → migration_pressure[nyc] → migration_in[boston] → population[boston]` is also enumerated with the analogous subscripted references.
- **ltm-503-cross-element-agg.AC2.4 Success:** The pure-A2A loop `population[r] → births[r] → population[r]` remains a single A2A `loop_score` variable with per-element slots (no regression from the cross-element changes).
- **ltm-503-cross-element-agg.AC2.5 Edge:** A model with no arrayed variables has its loops unchanged -- their `loop_score` equations reference unsubscripted scalar link scores exactly as before.

> Phase 2 depends on Phase 1 (link scores into arrayed targets must be meaningful before the loop product means anything).

---

## Context for the implementer (read before starting)

### How loop-score equations are built (the pipeline that matters — NOT `model_detected_loops`)

```
model_loop_circuits_tiered (db_analysis.rs:1934, #[salsa::tracked(returns(ref))])
   -> classifies cycles: CycleClass::PureScalar / PureSameElementA2A{dims} / CrossElementOrMixed (db_analysis.rs:1336/1396)
   -> pure cycles => fast_path: Vec<FastPathCircuit> {variables, dimensions} (db_analysis.rs:1864)
   -> cross-element/mixed => slow-path induced element subgraph -> SCC -> (if SCC<=MAX_LTM_SCC_NODES) Johnson -> slow_path: LoopCircuitsResult
   -> returns TieredCircuitsResult {fast_path, slow_path, slow_path_largest_scc} (db_analysis.rs:1899)
        |
build_loops_from_tiered (db_ltm.rs:1970)
   -> fast path: build one Loop directly per FastPathCircuit (circuit_to_links, find_stocks_in_loop, calculate_polarity, dims)
   -> slow path: build_element_level_loops(&tiered.slow_path, var_graph, source_vars, db, project, dm_dims) -> clear its ids -> extend
   -> assign_loop_ids(&mut all_loops)
        |
model_ltm_variables (db_ltm.rs:2450, #[salsa::tracked(returns(ref))])
   Part 1: emit $⁚ltm⁚link_score⁚... vars; collect names into emitted_link_score_names: HashSet<String>
   db_ltm.rs:2582-2626: model_loop_circuits_tiered + build_loops_from_tiered -> loops
   Part 2 (db_ltm.rs:~2989-3059): build CyclePartitions (from model_element_cycle_partitions);
       loop_partitions.insert(l.id, partitions.partition_for_loop(l))  [db_ltm.rs:3016]
       ltm_augment::generate_loop_score_variables(loops, &partitions, &emitted_link_score_names)  [db_ltm.rs:~3030]
       push the resulting vars carrying dimensions = Loop.dimensions onto LtmSyntheticVar  [db_ltm.rs:~3040-3057]
   Part 3: pathway / composite scores
        |
generate_loop_score_variables (ltm_augment.rs:486, pub(crate))
   per loop: var_name = "$⁚ltm⁚loop_score⁚{id}"; equation = generate_loop_score_equation(loop, emitted); create_aux_variable(&name, &equation)
   (also generates relative-loop-score vars using partitions)
        |
generate_loop_score_equation (ltm_augment.rs:965)
   maps loop.links -> resolve_link_score_name_for_loop(link.from, link.to, emitted) -> format!("\"{name}\"") -> join " * "
```

`create_aux_variable` (`ltm_augment.rs:991`) builds `datamodel::Variable::Aux { equation: datamodel::Equation::Scalar(equation_text), ... }`. (After Phase 1's `LtmSyntheticVar.equation: String -> datamodel::Equation` change, the conversion from this `datamodel::Variable` to the `LtmSyntheticVar` at `db_ltm.rs:3053` just stores the whole `Equation`.) **Phase 2 keeps the loop-score variable `Equation::Scalar` — only the equation *text* changes.**

The separate path `model_detected_loops` (`db_analysis.rs:1672`) feeds the FFI (`simlin_analyze_get_loops`) and diagram layout — **Phase 2 does NOT touch it.**

### Verified code locations (from codebase-investigator, 2026-05-09)

| Symbol | Location | Notes |
|---|---|---|
| `build_element_level_loops` | `src/simlin-engine/src/db_ltm.rs:2088` | `pub(crate) fn build_element_level_loops(element_circuits: &super::LoopCircuitsResult, var_graph: &crate::ltm::CausalGraph, source_vars: &HashMap<String, super::SourceVariable>, db: &dyn Db, project: SourceProject, dm_dims: &[crate::datamodel::Dimension]) -> Vec<crate::ltm::Loop>`. Groups circuits by variable-level node sequence; per group `representative = circuits_in_group[0]`, `all_subscripted = representative.iter().all(|n| n.contains('['))`. |
| `is_cross_element` predicate | `src/simlin-engine/src/db_ltm.rs:2167-2197` | for an `all_subscripted` group: repeated variable name (`has_repeated`) ⇒ true; else differing leading subscript element between consecutive nodes ⇒ true; else false. |
| Branch 1 — pure-A2A | `src/simlin-engine/src/db_ltm.rs:2199-2246` | `all_subscripted && !is_cross_element && !empty`: variable-level `links`/`stocks`, non-empty `dimensions`. **Untouched by Phase 2.** |
| Branch 2 — `is_cross_element` (THE REWRITE TARGET) | `src/simlin-engine/src/db_ltm.rs:2247-2320` | currently: strips subscripts, finds "shortest unique cycle" (`unique_cycle`/`seen_set` block at `~db_ltm.rs:2255-2270` — the "now-unused unique-cycle-stripping logic" to delete), `links = var_graph.circuit_to_links(&unique_cycle)` (variable-level diagonal names — **root cause of #503**), collects **element-level stocks** from `representative` (at `~db_ltm.rs:2280-2295` — **RETAIN this**), `Loop { id: "", links, stocks: element_stocks, polarity, dimensions: vec![] }`. |
| Branch 3 — mixed/scalar (the per-link construction to generalize) | `src/simlin-engine/src/db_ltm.rs:2321-2431` | per circuit: `var_links = var_graph.circuit_to_links(&stripped)`; builds element-subscripted links with the rule `let (link_from, link_to) = if from_subscripted && !to_subscripted { (from_raw, to_raw) } else { (strip_subscript(from_raw), strip_subscript(to_raw)) }`; `stocks` = element-level filter; `Loop { id: "", links, stocks, polarity, dimensions: vec![] }`. **Phase 2 needs a richer rule that also keeps `to[e_{i+1}]` so the loop-score equation can subscript an A2A link-score reference per element.** |
| `strip_subscript` | `src/simlin-engine/src/db_ltm.rs:1926-1931` | strips `[...]` to get variable-level name |
| `build_loops_from_tiered` | `src/simlin-engine/src/db_ltm.rs:1970` | caller of `build_element_level_loops`; merges fast + slow path; `assign_loop_ids` |
| `model_ltm_variables` (loop-score emission) | `src/simlin-engine/src/db_ltm.rs:2450`; tiered+build at `:2582`; `partition_for_loop` insert at `:3016`; `generate_loop_score_variables` call at `~:3030`; loop-score `LtmSyntheticVar` ctor at `:3053` | |
| `Link`, `Loop` structs | `src/simlin-engine/src/ltm/types.rs` | `Link { from: Ident<Canonical>, to: Ident<Canonical>, polarity: LinkPolarity }` — doc says cross-dim edges carry element-level `from` like `"pop[nyc]"`; `Link.to` currently always variable-level — **Phase 2 extends `Link.to` to carry `[elem]`**. `Loop { id: String, links: Vec<Link>, stocks: Vec<Ident<Canonical>>, polarity: LoopPolarity, dimensions: Vec<String> }` — `dimensions` non-empty ⇒ A2A loop score (per-element slots, `stocks` variable-level, `partition_for_loop` returns `None`); empty ⇒ scalar loop score, `stocks` MUST be element-level. `LoopPolarity`: `Reinforcing/Balancing/MostlyReinforcing/MostlyBalancing/Undetermined`. `LinkPolarity`: `Positive/Negative/Unknown`. |
| `circuit_to_links` / `find_stocks_in_loop` / `calculate_polarity` / `get_link_polarity` | `src/simlin-engine/src/ltm/graph.rs:461 / 670 / 830 / 773` | |
| `assign_loop_ids` | `src/simlin-engine/src/ltm/graph.rs:905` | sorts loops by sort key = sorted+deduped `link.from`/`link.to` strings joined `_`, then assigns `r{n}`/`b{n}`/`u{n}` by polarity. **CAVEAT: element-subscripted link names change the sort key ⇒ can reorder which loop gets `r1`. Tests look for `$⁚ltm⁚loop_score⁚r1` (`\u{205A}r1`). Re-verify ID stability, or normalize the sort key to strip subscripts for cross-element loops so IDs stay stable.** |
| `generate_loop_score_equation` | `src/simlin-engine/src/ltm_augment.rs:965` | `fn generate_loop_score_equation(loop_item: &Loop, emitted_link_score_names: &HashSet<String>) -> String`. Today: per link, `resolve_link_score_name_for_loop(link.from, link.to, emitted)` ⇒ `format!("\"{name}\"")` ⇒ join `" * "`; empty ⇒ `"0"`. **No `[elem]` subscript today.** |
| `resolve_link_score_name_for_loop` | `src/simlin-engine/src/ltm_augment.rs:904` | `pub(crate) fn resolve_link_score_name_for_loop(from: &str, to: &str, emitted: &HashSet<String>) -> String`. Tries Bare name; then `find_fixed_index_emitted_name`; then Wildcard suffix; then Dynamic suffix; falls back to Bare. **Add `target_element: Option<&str>` param (or sibling fn).** |
| `find_fixed_index_emitted_name` | `src/simlin-engine/src/ltm_augment.rs:930` | scans emitted for `$⁚ltm⁚link_score⁚{from}[<something>]→{to}`, returns first alphabetically (heuristic). The `target_element` param lets it match `{from}[{target_element}]→{to}` exactly. |
| `link_score_var_name` | `src/simlin-engine/src/ltm_augment.rs:458` | `FixedIndex(elems)` ⇒ `{from}[{elems.join(",")}]→{to}`; `Wildcard`/`DynamicIndex` ⇒ `{to}⁚wildcard`/`{to}⁚dynamic`; `Bare` ⇒ `{from}→{to}`. **No variant that subscripts the `to` side with `[elem]`** (only the suffixes do). |
| `generate_loop_score_variables` | `src/simlin-engine/src/ltm_augment.rs:486` | `pub(crate) fn generate_loop_score_variables(loops: &[Loop], partitions: &CyclePartitions, emitted_link_score_names: &HashSet<String>) -> HashMap<Ident<Canonical>, datamodel::Variable>` |
| `create_aux_variable` | `src/simlin-engine/src/ltm_augment.rs:991` | builds `datamodel::Variable::Aux { equation: Equation::Scalar(text), ... }` — **loop-score var stays `Scalar`** |
| `partition_for_loop` | `src/simlin-engine/src/ltm/partitions.rs:42` | `loop_item.stocks.iter().find_map(|s| self.stock_partition.get(s).copied())`. **No change in Phase 2 — keep feeding it element-level `Loop.stocks`.** |
| `model_element_cycle_partitions` | `src/simlin-engine/src/db_analysis.rs:2202` | `#[salsa::tracked(returns(ref))]`; keyed on element-level names like `"population[nyc]"`. **No change.** |
| `model_detected_loops` (NOT the loop-score path) | `src/simlin-engine/src/db_analysis.rs:1672` | feeds FFI + diagram layout. **Do not touch.** |
| `cross_element_ltm` fixture | `test/cross_element_ltm/cross_element.stmx` | Region={NYC,Boston}, euler 0..50 dt 1. `population` per-element stock (NYC=1000, Boston=500; inflows `births`, `migration_in`; outflow `migration_out`). `births` A2A `population * 0.02`. `migration_pressure` per-element aux: NYC `(population[NYC] - population[Boston]) * 0.01`, Boston `(population[Boston] - population[NYC]) * 0.01`. `migration_out` A2A `MAX(migration_pressure, 0)`. `migration_in` per-element flow: NYC `MAX(migration_pressure[Boston] * -1, 0)`, Boston `MAX(migration_pressure[NYC] * -1, 0)`. `total_population` scalar `SUM(population[*])`. No `ltm_results.tsv`. |

### Tests touched / added

| Test | Location | Change |
|---|---|---|
| `test_cross_element_ltm_exhaustive` | `src/simlin-engine/tests/simulate_ltm.rs:4404` | has a deliberately-relaxed `any_loop_active` assertion at `~:4509-4537` (comments cite "tech-debt-#34" / "slot-0 broadcast bug") AND an already-partially-tightened block at `~:4539-4567` (asserts both NYC & Boston slots of the `\u{205A}r1` A2A births loop are non-zero; a comment notes `MAX(...)`-induced zero slots in the migration `u*` loops are legitimate). Phase 2 tightens further: assert the cross-element migration loop's loop-score equation references the *swap* link scores (`migration_pressure[boston]→migration_in[nyc]` etc.), not the diagonal `migration_out` link score, and (where the fixture's pressure signs keep all links non-zero) the loop-score *value* equals the product of the per-element link scores along the element path. |
| `test_cross_element_ltm_edge_set_truthful` | `src/simlin-engine/tests/simulate_ltm.rs:4583` | pins the truthful per-reference element edge set: `population[nyc/boston]→migration_pressure[nyc/boston]` (all 4), `migration_pressure[boston]→migration_in[nyc]` + `migration_pressure[nyc]→migration_in[boston]` (the swap pair), `assert_no_edge` same-element `migration_pressure→migration_in`, A2A diagonals for `migration_out` & `births`, `population[nyc/boston]→total_population`, flow→stock edges. Phase 2's loop-score equations must walk exactly these edges. **Should keep passing unchanged.** |
| `measurement_postscript_cross_element_ltm` | `src/simlin-engine/tests/simulate_ltm.rs:3982` | loose (`m.fast_path >= 1`, `m.slow_path_scc <= m.elem_scc`). Phase 6 updates the postscript doc; if the SCC/loop count shifts, this may need updating — note it for Phase 6. |
| `test_a2a_pure_dimension_loop_scores` (AC2.4) | `src/simlin-engine/tests/simulate_ltm.rs:3104` | 3-region pure-A2A; asserts exactly 1 loop-score var, n_elements slots all non-zero, 1 loop id in partition map, rel scores ±1.0. Helper `find_loop_score_offsets` at `simulate_ltm.rs:3064`. **Must keep passing.** |
| `a2a_loop_links_use_variable_level_names` (AC2.4) | `src/simlin-engine/src/db_ltm_unified_tests.rs:961` | pure-A2A `pop[r]→births[r]→pop[r]`, asserts every `Link.from`/`Link.to` is variable-level (no `[`). **Must keep passing.** |
| `cross_element_loop_partitions_resolve_to_some` / `mixed_scalar_loop_partitions_resolve_to_some` | `src/simlin-engine/src/db_ltm_unified_tests.rs:1261 / 1341` | scalar/cross-element loops (`dimensions` empty) resolve to `Some` partition. **Must keep passing** (cross-element loops keep element-level stocks). |
| `cross_element_loop_through_sum_reducer` / `cross_element_stocks_in_same_partition` | `src/simlin-engine/src/db_element_graph_tests.rs:449 / 488` | `population[Region]` (2-region stock, inflow `births`) + `births[Region] = SUM(population[*]) * 0.01`; asserts exactly 3 circuits (2 same-element + 1 cross-element 4-node `[population[nyc], births[boston], population[boston], births[nyc]]`) and both `population` elements in the same partition. **Should keep passing** (this is a circuit-enumeration test, not a loop-score test). |
| AC2.5 — scalar-model loop-score test | (add) | no single existing dedicated test; add a small one: scalar-only model with a feedback loop, assert the `$⁚ltm⁚loop_score⁚...` equation text is the product of bare quoted link-score names with **no `[elem]` subscripts**. Or extend an existing scalar-loop test in `db_ltm_unified_tests.rs` (`mixed_scalar_loop_score_refs_resolve_to_emitted_names` at `:1011` is a candidate sibling). |

### Conventions

TDD mandatory. Cross-element loop-score equation assertions: `db_ltm_unified_tests.rs` (build a `TestProject`, enable LTM, fetch `model_ltm_variables`, find the `$⁚ltm⁚loop_score⁚...` var, assert on its equation `datamodel::Equation::Scalar` text). End-to-end value checks: `simulate_ltm.rs` (compile-and-simulate, find loop-score offsets, compare to a hand calc). `TestProject` builder is in `src/simlin-engine/src/test_common.rs` (`.named_dimension`, `.array_stock`, `.array_aux`, `.scalar_aux`, `.array_flow`, `.array_with_ranges` for `Equation::Arrayed`, `.with_sim_time`, `.build_datamodel`). Enable LTM via `use salsa::Setter; ...set_ltm_enabled(&mut db).to(true)` or `set_project_ltm_enabled`. Each new unit test under ~2s; `cargo test --workspace` under the 3-minute cap. Commits: `engine: ...`, no emoji, no `Co-Authored-By`, never `--no-verify`.

---

## Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: `target_element`-aware loop-score link-score reference resolution

**Verifies:** (foundation for AC2.1, AC2.3; no behavior change on its own — gate is "existing tests stay green")

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` — `resolve_link_score_name_for_loop` (`:904`), `find_fixed_index_emitted_name` (`:930`), `generate_loop_score_equation` (`:965`).
- Test: `src/simlin-engine/src/ltm_augment.rs` `#[cfg(test)] mod tests` (unit).

**Implementation contract:**
- Add a `target_element: Option<&str>` parameter to `resolve_link_score_name_for_loop` (or a sibling fn the caller uses). When `target_element` is `Some(e)`:
  - `find_fixed_index_emitted_name` matches `$⁚ltm⁚link_score⁚{from}[{e}]→{to}` exactly (when the loop edge's `from` carries `[e_from]`) rather than guessing alphabetically.
  - Resolution returns the name **and** signals whether the resolved variable is a *dimensioned* (A2A) link score that must be subscripted `[e]` at the reference site. (Cleanest shape: return `(name: String, subscript: Option<String>)`, where `subscript` is the target-element string when the resolved var is A2A and the loop edge visits a specific element.)
- `generate_loop_score_equation`: for each loop edge, pass the element the loop visits at the `to` node (parse it off `Link.to` if `Link.to` carries `[e]`, after Task 2 makes that the encoding). If the resolved link score is dimensioned and the loop edge has an element, emit `"$⁚ltm⁚link_score⁚{from}→{to}"[e]` (the var name double-quoted, then `[e]` outside the quotes — verify the equation parser accepts `"quoted name"[elem]` subscript syntax; it must, since A2A link scores referenced elementwise already need this form somewhere — check how `relative loop score` / per-element references are spelled today). Otherwise emit the bare quoted name exactly as before.
- The function for the `RefShape::Wildcard`/`RefShape::DynamicIndex` fallbacks stays as-is for now (Phase 6 removes them).
- **No behavior change yet:** with `target_element = None` everywhere (the only caller, until Task 2 changes the loop construction), the output is byte-identical to today. Existing loop-score tests stay green.

**Testing:** TDD. Unit tests on `resolve_link_score_name_for_loop` / `generate_loop_score_equation`: (a) with `target_element = None`, output equals today's behavior (regression guard); (b) with `target_element = Some("boston")` and an emitted set containing `$⁚ltm⁚link_score⁚population[nyc]→migration_pressure` with that var dimensioned over `Region`, the reference is `"$⁚ltm⁚link_score⁚population[nyc]→migration_pressure"[boston]`; (c) with a scalar→arrayed-style emitted name, see Phase 3 (out of scope here — Task 1 just handles the A2A-subscript case).

**Verification:** `cargo test -p simlin-engine ltm_augment` — new tests pass, existing pass. `cargo test -p simlin-engine` — green (no behavior change anywhere).

**Commit:** `engine: add target_element resolution for loop-score link refs`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Rewrite the `is_cross_element` branch of `build_element_level_loops`

**Verifies:** ltm-503-cross-element-agg.AC2.1, ltm-503-cross-element-agg.AC2.2, ltm-503-cross-element-agg.AC2.3, ltm-503-cross-element-agg.AC2.4, ltm-503-cross-element-agg.AC2.5

**Files:**
- Modify: `src/simlin-engine/src/db_ltm.rs` — `build_element_level_loops` (`:2088`), specifically Branch 2 (`is_cross_element`, `:2247-2320`): delete the `unique_cycle`/`seen_set` "shortest unique cycle" block (`~:2255-2270`) and the `circuit_to_links(&unique_cycle)` diagonal-name call; replace with per-circuit element-subscripted `Link` construction (generalize Branch 3's loop). **Retain** the element-level stock collection (`~:2280-2295`).
- Possibly modify: `src/simlin-engine/src/ltm/graph.rs` — `assign_loop_ids` (`:905`) sort key, if loop-ID stability for `$⁚ltm⁚loop_score⁚r1`-dependent tests requires normalizing the key to strip subscripts.
- Possibly modify: `src/simlin-engine/src/db_ltm.rs` — the loop-score emission in `model_ltm_variables` Part 2 (`~:3030`) only if `generate_loop_score_variables`'s call signature changes to thread `target_element` (it gets the element from `Link.to`, so likely no signature change — confirm).
- Test: `src/simlin-engine/src/db_ltm_unified_tests.rs` (loop-score-equation assertions), `src/simlin-engine/tests/simulate_ltm.rs` (end-to-end values).

**Implementation contract:**
- For an `is_cross_element` group, build **one `Loop` per circuit** (like Branch 3), with element-subscripted `Link`s. The per-link rule generalizes Branch 3's: for circuit nodes `[n_0, n_1, ..., n_{k-1}]` (each `n_i` either `var` or `var[e_i]`), build `Link { from: <element-or-var name of n_i>, to: <element-or-var name of n_{i+1}>, polarity: <variable-level polarity for that hop> }` where:
  - if `n_i` is subscripted, keep `from = n_i` (e.g. `"population[nyc]"`);
  - if `n_{i+1}` is subscripted AND the corresponding link-score variable is A2A (dimensioned over the target's dims), keep `to = n_{i+1}` (e.g. `"migration_pressure[boston]"`) so `generate_loop_score_equation` can emit `"$⁚ltm⁚link_score⁚{from-name}→{to-name}"[e_{i+1}]`;
  - else `to = strip_subscript(n_{i+1})`.
  - (Determining "is the link-score variable A2A" at this point: you have access to the source vars / `var_graph`; check whether the target variable is dimensioned. A target like `migration_in` referenced from `migration_pressure[boston]` produces `$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in` with `dimensions=["Region"]` (Phase 1 made its equation `Equation::Arrayed`) — so yes, subscript it `[nyc]` at the visited element. A target like `population` (a stock) referenced from `migration_in` produces `$⁚ltm⁚link_score⁚migration_in→population` — is *that* A2A? `population` is a per-element stock (`Equation::Arrayed`); the flow→stock link score... check what `generate_flow_to_stock_equation` produces for an arrayed stock. If it's A2A over `Region`, subscript it `[nyc]`. AC2.1's expected reference is `"$⁚ltm⁚link_score⁚migration_in→population"[nyc]`, so yes it's dimensioned.)
- `Loop.dimensions` stays `vec![]` for cross-element loops (the loop-score *variable* is scalar). `Loop.stocks` stays **element-level** (collected from the circuit's stock nodes — keep the existing collection logic). `Loop.polarity` from `var_graph.calculate_polarity(&var_links)` as today.
- Delete the now-dead "shortest unique cycle" stripping and the diagonal `circuit_to_links` call. Do not leave the old code commented out.
- If `assign_loop_ids`'s sort key (which dedups `link.from`/`link.to` strings) now produces different orderings because cross-element loops carry element-subscripted names, and a test depends on a specific loop being `r1`/`b1`/`u1`, normalize the sort key for the *purpose of ID assignment only* by stripping subscripts before building the key (the IDs are just labels; the loop content is unchanged). Decide based on whether `cargo test --workspace` shows ID churn.
- Pure-A2A loops (Branch 1) and the mixed/scalar Branch 3 are untouched (except that Branch 3's per-link rule might be refactored into a shared helper that Branch 2 reuses — fine, as long as Branch 3's *behavior* is unchanged: a "mixed" loop where the `to` node is a same-element A2A still gets `to` stripped, because there's no cross-element reference there).

**Testing:** TDD. Tests must verify each AC:
- AC2.1: `cross_element_ltm` fixture (or an equivalent `TestProject` model) — enable LTM (exhaustive mode), fetch `model_ltm_variables`, find the loop-score var for the loop whose links are `population[nyc] → migration_pressure[boston] → migration_in[nyc] → population[nyc]` (identify by inspecting `model_detected_loops` or the loop-score var's equation contents), assert its `Equation::Scalar` text contains `"$⁚ltm⁚link_score⁚population[nyc]→migration_pressure"[boston]`, `"$⁚ltm⁚link_score⁚migration_pressure[boston]→migration_in"[nyc]`, `"$⁚ltm⁚link_score⁚migration_in→population"[nyc]` (the exact spelling of the subscript syntax — confirm against how the parser/`print_eqn` renders a quoted-name subscript; assert on a canonical form) — and does NOT contain the unsubscripted A2A diagonal names where the loop visits a specific element.
- AC2.2: compile-and-simulate the `cross_element_ltm` fixture; find the `$⁚ltm⁚loop_score⁚...` offset for that loop; at >= 1 timestep, compute the expected value by hand (product of the per-element link scores along the path — read those link scores' values from the same `Results` and multiply) and assert the loop-score value matches within `1e-6`. (Choose a timestep where the fixture's `migration_pressure` signs keep all three links non-zero — e.g. NYC's pressure is positive throughout (1000 > 500), so the NYC→migration_pressure→migration_in→population loop... wait, check: `migration_in[NYC] = MAX(migration_pressure[Boston]*-1, 0)`; `migration_pressure[Boston] < 0` ⇒ `migration_in[NYC] > 0`; pick the loop direction where every link is active.)
- AC2.3: the symmetric loop `population[boston] → migration_pressure[nyc] → migration_in[boston] → population[boston]` — same kind of assertion. (Note: if the fixture's dynamics make one of its links identically zero — e.g. `migration_in[Boston] = MAX(migration_pressure[NYC]*-1, 0) = MAX(-5,0) = 0` constantly — then the loop is *enumerated* and its equation references the right subscripted link scores, but its *value* is zero; assert the enumeration + equation references, and document that the value is zero by the fixture's `MAX(...)` semantics. The design's AC2.3 only requires "enumerated with the analogous subscripted references".)
- AC2.4: `test_a2a_pure_dimension_loop_scores` (`simulate_ltm.rs:3104`) and `a2a_loop_links_use_variable_level_names` (`db_ltm_unified_tests.rs:961`) still pass unchanged — a pure-A2A loop stays one A2A loop-score var with per-element slots, all `Link`s variable-level.
- AC2.5: add (or extend) a scalar-only-model loop-score test asserting the equation text is the product of bare quoted link-score names with no `[elem]`.
- Also: `test_cross_element_ltm_exhaustive` — tighten the relaxed `any_loop_active` assertion as described in the "Tests touched" table (assert the migration loop uses the swap link scores, not the diagonal; assert the loop-score value where the fixture keeps all links non-zero). `test_cross_element_ltm_edge_set_truthful` — keep passing. `measurement_postscript_cross_element_ltm` — keep passing (note any count shift for Phase 6).

**Verification:**
- Run: `cargo test -p simlin-engine` — green.
- Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm` — green; the new AC2.x tests pass; `test_cross_element_ltm_*`, `test_a2a_*`, `measurement_postscript_*` pass.
- Run: `cargo test --workspace` — green within the 3-minute cap.

**Commit:** `engine: score cross-element loops on the element-level path`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase 2 done-when checklist

- [ ] `resolve_link_score_name_for_loop` (or a sibling) takes a `target_element`; `generate_loop_score_equation` emits `"$⁚ltm⁚link_score⁚{from}→{to}"[e]` for dimensioned link scores along an element path.
- [ ] `build_element_level_loops`'s `is_cross_element` branch keeps element subscripts on each `Link`; the diagonal `circuit_to_links(&unique_cycle)` collapse and the "shortest unique cycle" stripping are deleted; element-level stocks for `partition_for_loop` are retained.
- [ ] AC2.1, AC2.3 — the two cross-element migration loops are enumerated with the subscripted link-score references.
- [ ] AC2.2 — the loop-score series matches a hand calc at >= 1 step within 1e-6.
- [ ] AC2.4 — pure-A2A loops unchanged (`test_a2a_pure_dimension_loop_scores`, `a2a_loop_links_use_variable_level_names` pass).
- [ ] AC2.5 — scalar-model loop-score equations unchanged (no `[elem]` subscripts).
- [ ] `test_cross_element_ltm_exhaustive`'s relaxed assertion tightened; `test_cross_element_ltm_edge_set_truthful` and `measurement_postscript_cross_element_ltm` still pass (note any enumeration-count shift for Phase 6's postscript update).
- [ ] Loop IDs stable for tests that depend on `$⁚ltm⁚loop_score⁚r1` (normalize `assign_loop_ids` sort key if needed).
- [ ] `cargo test --workspace` green within the 3-minute cap; clippy/fmt clean (run `git commit` and let the pre-commit hook gate it).
