# Single-mode LTM Design

## Summary

Simlin implements Loops That Matter (LTM), a method that measures how much each feedback loop in a system dynamics model accounts for the model's behavior at each point in time. The engine does this two different ways today. The exhaustive path enumerates loops at compile time and synthesizes one extra simulation variable per loop, which computes that loop's score as the simulation runs. The discovery path instead instruments every causal link and reconstructs the loops afterward from the recorded link-score series. A model-size gate picks between them automatically, and both of the large real models the project measures itself against (World3 and C-LEARN) trip it. So the exhaustive path only ever runs on small test fixtures, every consumer that reads its compile-time loop list is empty on the models that matter, and the TypeScript engine has no binding to the other path at all.

This plan deletes the exhaustive path and makes the post-simulation pipeline the only one. An audit established that the post-simulation product of a loop's link scores reproduces the in-simulation loop-score column to one ulp on all seven LTM fixtures, so the in-simulation form carries nothing worth keeping; those columns are captured as goldens before the code producing them is removed, and the replacement is asserted against them. Collapsing to one mode also removes the reason six decisions each had two implementations (cycle partitions, loop deduplication, score normalization, polarity, the per-exit-port module override, and cross-aggregate loop recovery), so most phases are consolidations onto a single owner rather than new machinery. Along the way the plan drops a sampling fallback whose measured recall was 0.20 on the only model dense enough to need it, replaces compile-time polarity analysis with classification from the run, makes an inline array reducer a real reported node instead of stitching loops back together around it, and stops emitting module instrumentation for instances that cannot lie on a loop. The bar for landing is that every surface reports loops on every model, the scores match the retained goldens, and compile time, run time and slot count stay within five percent of today's figures.

## Definition of Done

Each item states what is true of the tree when this plan has landed, with the command or test that shows it.

1. **One instrumentation.** An LTM compile scores every causal edge of every model; there is no loop-edge-only instrumentation, no `$⁚ltm⁚loop_score⁚` synthetic variable, no `ltm_discovery_mode` input, no `LtmMode` enum, no auto-flip gate and no auto-flip warning. `rg "loop_score⁚|ltm_discovery_mode|LtmMode|MAX_LTM_SCC_NODES|auto-switched" src/` is empty outside `docs/`.
2. **One loop-scoring pipeline.** Every consumer that reports loop scores (libsimlin, pysimlin, MCP, `@simlin/engine`, the CLI `--ltm` report, the layout) reads them from one post-simulation pipeline over the recorded link-score series (`ltm_finding::discover_loops_with_graph` and its successors). Per-loop, per-slot raw and partition-relative series are bit-identical to the retained goldens of the seven LTM fixtures captured from the exhaustive `loop_score` columns before this plan lands (`tests/integration/ltm_single_mode_parity.rs`).
3. **Every surface gets loops on every model.** World3 and C-LEARN report loops through libsimlin `simlin_analyze_get_loops_runtime`, pysimlin `Run.loops`, MCP `read_model`, `@simlin/engine` `Run.loops`, and the CLI `--ltm` report, with the same ids and scores on each (`tests/integration/ltm_discovery_large_models.rs`, `src/engine/tests/wasm-ltm.test.ts`, `src/pysimlin/tests/test_ltm.py`).
4. **Pins are reported regardless of retention or the cap**, scored by the same pipeline, with the user's name and a stable id (`tests/integration/simulate_ltm_pinned.rs`).
5. **The shortest-path fallback is deleted.** On budget exhaustion the partial universe is ranked and reported with `enumeration_complete == false`; `src/simlin-engine/src/ltm_finding_fallback.rs`, its tests, `examples/ltm_fallback_eval.rs`, `FallbackConfig`, `ENUM_BUDGET_FRACTION` and `fallback_candidates` do not exist.
6. **One owner per decision.** Cycle partitions come from `db::analysis::model_element_cycle_partitions`; loop identity from one element-level canonical key in `ltm/mod.rs`; relative normalization from one function in `ltm_post.rs`; runtime polarity from `LoopPolarity::from_runtime_scores` over the relative series; the per-exit-port module override from `ltm_finding.rs`; each verified by `rg` in the phase that consolidates it.
7. **An inline reducer is a node.** A loop through `SUM(pop[*])` is an elementary circuit of the element graph in which the reducer's `$⁚ltm⁚agg⁚{n}` node is a real, reported node (displayed as the reducer's spelling); there is no petal stitching, no `MAX_AGG_PETALS`, no `MAX_CROSS_AGG_LOOPS`, no `agg_recovery_truncated`. The three-element `test/cross_agg_ltm` fixture reports three loops with relative scores summing to 1 (`tests/integration/ltm_array_agg.rs`).
8. **Static polarity is gone.** `ltm/polarity.rs` and its tests do not exist; a loop's polarity is a field derived from its runtime relative series, `None` before a run; loop ids carry no polarity letter.
9. **Sub-model pathway and composite scores are emitted only for module instances that can lie on a feedback loop** (an instance whose node is in a nontrivial SCC of its parent's causal graph). `Theil_2011.mdl` (zero loops, many SMOOTH instances) compiles under the overlay to at most 1.5x its plain slot count (`examples/ltm_full_bench.rs`).
10. **Cost and memory on the corpus are not worse than today.** For every model in `docs/design/engine-performance.md`'s ledger the LTM compile time, run time and slot count are within 5 percent of the pre-plan figures, and C-LEARN's discovery pass stays under 100 ms (ledger rows in this document).

## Acceptance Criteria

Drafted from the Definition of Done; to be validated with the owner before an implementation plan is written from them.

### ltm-single-mode.AC1: One instrumentation
- **ltm-single-mode.AC1.1 Success:** `model_ltm_variables` on `test/logistic_growth_ltm` emits exactly the seven link-score variables of today's discovery instrumentation and no loop-score variable.
- **ltm-single-mode.AC1.2 Success:** Compiling World3 under the overlay emits no diagnostic (today: the auto-flip warning).
- **ltm-single-mode.AC1.3 Success:** `SourceProject` has no `ltm_discovery_mode` field; `analyze_model` sets no input and opens no revision (`db::exec_probe::ProbedDb` shows zero re-executed `model_ltm_variables` bodies on a second `analyze_model` call on the same db).
- **ltm-single-mode.AC1.4 Failure:** A model whose LTM compile fails (an unfreezable partial in every link of a loop) still simulates plainly; the failure is one `Warning` naming the link, never an empty loop list without a reason.

### ltm-single-mode.AC2: One loop-scoring pipeline
- **ltm-single-mode.AC2.1 Success:** For each of the seven fixtures (`logistic_growth_ltm`, `arms_race_3party`, `decoupled_stocks`, `arrayed_population_ltm`, `cross_element_ltm`, `cross_agg_ltm`, `hero_culture_ltm`), the post-simulation raw loop score of every loop at every saved step equals the retained exhaustive golden to within one ulp, and the partition-relative score equals the golden recomputed under per-partition normalization.
- **ltm-single-mode.AC2.2 Success:** libsimlin, pysimlin, MCP and `@simlin/engine` report identical loop ids, polarities and relative series for `hero_culture` (cross-surface test).
- **ltm-single-mode.AC2.3 Success:** The layout's loop importance (`layout::detect_ltm_loops`) is computed from the same results the display run produces, not from a second simulation (`ProbedDb` shows one `Vm::run_to_end` per layout on a loop-bearing model).
- **ltm-single-mode.AC2.4 Edge:** A stateless model (no stocks, no lagged deps) emits no LTM variables and reports an empty loop list with `enumeration_complete == true`.

### ltm-single-mode.AC3: Every surface, every model
- **ltm-single-mode.AC3.1 Success:** World3 reports 200 loops (the coverage-aware cap) on every surface; C-LEARN reports 153.
- **ltm-single-mode.AC3.2 Success:** `@simlin/engine` `Model.run({analyzeLtm: true})` on World3 under the `'vm'` and `'wasm'` engines returns the same loops as libsimlin (the wasm engine reaches the pipeline through `simlin_analyze_discover_loops_from_wasm_results`).
- **ltm-single-mode.AC3.3 Success:** pysimlin `Model.run()` on World3 returns a non-empty `Run.loops` and emits no `RuntimeWarning`.
- **ltm-single-mode.AC3.4 Success:** `Run.links` on the TS surface excludes `$⁚` internal nodes by default (`includeInternal` defaults to false, matching pysimlin).

### ltm-single-mode.AC4: Pins
- **ltm-single-mode.AC4.1 Success:** A pinned loop below `MIN_CONTRIBUTION` at every step is still reported, with its relative series (near 0) and its user name.
- **ltm-single-mode.AC4.2 Success:** A pinned loop that the enumerator also finds is reported once, under the pin's name, with the enumerator's id.
- **ltm-single-mode.AC4.3 Failure:** A pin naming a variable set with no closed cycle is reported in `PinnedLoopsResult::invalid` with the pin's identity, as today.
- **ltm-single-mode.AC4.4 Success:** An arrayed pin reports one loop per element instance, each with its own series.

### ltm-single-mode.AC5: Fallback deleted
- **ltm-single-mode.AC5.1 Success:** With `MAX_DISCOVERY_ENUM_CIRCUITS` lowered by the test-only `EnumBudgetGuard` to a value below World3's universe, discovery reports `enumeration_complete == false`, a non-empty ranked list drawn from the partial universe, and `universe_loops == None`.
- **ltm-single-mode.AC5.2 Success:** The wall-clock budget is spent entirely on the enumeration; an expired deadline yields the partial result of AC5.1.

### ltm-single-mode.AC6: One owner
- **ltm-single-mode.AC6.1 Success:** `rg -n "compute_cycle_partitions|model_cycle_partitions\b" src/simlin-engine/src` finds only `model_element_cycle_partitions` and its readers.
- **ltm-single-mode.AC6.2 Success:** Two circuits that are rotations of each other, in exhaustive Johnson output, in a pin expansion and in discovery, all resolve to one key from `ltm::canonical_cycle_key`.
- **ltm-single-mode.AC6.3 Success:** The relative series of any loop, read through libsimlin's per-element accessor, pysimlin, and the layout, are the same bytes (one normalization function).

### ltm-single-mode.AC7: Reducer is a node
- **ltm-single-mode.AC7.1 Success:** `test/cross_agg_ltm` reports exactly three loops, `pop[x] -> SUM(pop) -> growth[x] -> pop[x]` for x in {a, b, c}, each with raw score 1/3 and relative score 1/3.
- **ltm-single-mode.AC7.2 Success:** The de-subscripted twin of that model with the SUM named (`total = SUM(pop[*])`) reports the same three loops with the same scores (the oracle in the audit's scratchpad, promoted to `tests/integration/ltm_desubscript_oracle.rs`).
- **ltm-single-mode.AC7.3 Success:** A `migration_matrix`-class model (3x3 flow matrix through one reducer) completes with no truncation flag and a loop count linear in the element count.

### ltm-single-mode.AC8: Static polarity gone
- **ltm-single-mode.AC8.1 Success:** Before a run, `Model.get_loops()` (pysimlin) and `simlin_analyze_get_loops` report loops with `polarity == None` and ids without a letter prefix; after a run, every loop in the corpus carries `Reinforcing` or `Balancing` with confidence 1.0 except the yeast model's R loop (`Undetermined`).
- **ltm-single-mode.AC8.2 Success:** A loop's id is the same before and after the run and across two runs with different parameters.

### ltm-single-mode.AC9: Module emission gated on loop membership
- **ltm-single-mode.AC9.1 Success:** `Theil_2011.mdl` under the overlay has at most 1.5x its plain slot count and emits no pathway or composite variables.
- **ltm-single-mode.AC9.2 Success:** `smooth3.mdl` (a SMOOTH inside a loop) still emits the pathway and composite variables of the instances on the loop, and their loop scores match the retained goldens.

### ltm-single-mode.AC10: Cost
- **ltm-single-mode.AC10.1 Success:** The ledger rows for every corpus model are within 5 percent of the pre-plan compile, run and slot figures; C-LEARN discovery under 100 ms.

## Glossary

- **LTM (Loops That Matter)**: The feedback-loop dominance method Simlin implements (Schoenberg, Eberlein et al.). It quantifies, at each timestep, how much of a model's observed behavior each feedback loop accounts for. Full write-up in `docs/reference/ltm--loops-that-matter.md`.
- **Link score**: A dimensionless per-timestep measure of one causal link's contribution to the change in its target variable, carrying both a magnitude and a sign. Computed by re-evaluating the target's equation with every input except the one under test held at its previous value.
- **Loop score**: The product of the link scores around a closed loop. Its sign is the loop's polarity; its magnitude is the "force" the loop exerts on the stocks it touches. A loop alone on its stocks always scores exactly plus or minus 1.
- **Relative loop score**: A loop score normalized by the sum of absolute loop scores of all loops in its cycle partition. Lands in [-1, 1], and the absolute values within a partition sum to 1. This is what consumers plot, because raw scores diverge toward infinity near dominance shifts where competing loops cancel.
- **Polarity (reinforcing / balancing / undetermined)**: An even number of negative links makes a loop reinforcing; an odd number makes it balancing; a link of unknown sign makes it undetermined. Static polarity is derived from the equation AST at compile time; runtime polarity is classified from the sign of the recorded score series, with a confidence value. This plan deletes static polarity.
- **Cycle partition**: A group of stocks connected to each other through feedback, computed as a strongly connected component of the stock-to-stock reachability graph. Relative scores are only meaningful within a partition.
- **SCC (strongly connected component)**: A maximal set of graph nodes where every node reaches every other. Every feedback loop lies entirely inside one SCC, so a node in no nontrivial SCC cannot be on a loop.
- **Element graph**: The causal graph after arrayed variables are expanded to one node per array element, so `pop[nyc]` and `pop[boston]` are distinct nodes and reported loops are element-specific.
- **Slot**: One element position of an arrayed variable, and equivalently one output column of a run. "Slot count" is the proxy for how much the LTM overlay inflates a compiled model.
- **A2A (apply-to-all), and the A2A collapse**: Apply-to-all is the ordinary arrayed equation form, one equation covering every element diagonally. Today's exhaustive path collapses such a family into a single `Loop` carrying per-slot link lists (`slot_links`); this plan reports N separate element-level loops that share a variable-level shape instead.
- **Aggregate node (`$⁚ltm⁚agg⁚{n}`)**: An analysis-only stand-in for an inline array reducer such as the `SUM(pop[*])` inside `share[r] = pop[r] / SUM(pop[*])`. Causality is routed through it rather than scored as one lumped link. Model equations are never rewritten.
- **Reducer**: A builtin that collapses an array dimension: `SUM`, `MEAN`, `MIN`, `MAX`, `STDDEV`, `RANK`, `SIZE`.
- **Petal stitching**: The current mechanism for loops that visit one aggregate node more than once. It enumerates `agg -> ... -> agg` segments ("petals") and emits one loop per pairwise-disjoint subset, which is exponential in the petal count, needs budgets and a truncation flag, and matches neither the de-subscripted model's loop set nor the named-aggregate model's. Deleted by this plan.
- **Module instance / sub-model**: A nested model used as a variable, either a stdlib macro (`SMOOTH`, `DELAY`, `TREND`) or a user-defined sub-model. Its inputs and outputs are entry and exit ports.
- **Pathway score**: The product of link scores along one internal path through a module instance, from an entry port to an exit port.
- **Composite score**: The pathway score with the largest absolute magnitude at each timestep. It serves as the parent model's link score for the edge into the instance, hiding the instance's internals the way the papers hide a macro's.
- **Per-exit-port override**: A correction that rescores a loop's module link against the pathway ending at the port the loop actually leaves through, rather than the composite's max-magnitude pick across all ports.
- **Pin (pinned loop)**: A loop a modeler names explicitly by its variable set. The reference calls this `LOOPSCORE`: the named loop is reported and scored regardless of what discovery found. Pins keep `pin{n}` ids and are exempt from retention and from the report cap.
- **Elementary circuit**: A directed cycle that visits no node twice; what "a loop" means algorithmically.
- **Johnson's algorithm**: The standard output-sensitive enumerator for all elementary circuits of a directed graph. Used at compile time to produce the structural loop list.
- **Structural loops**: The pre-run loop list, produced by a budgeted Johnson run over the element graph. After this plan it supplies loop identity and ids only, never scores.
- **Canonical cycle key**: A rotation-invariant identity for a cycle, preserving direction. `A -> B -> C -> A` and `B -> C -> A -> B` collapse to one key, while `A -> C -> B -> A` stays a distinct loop (GH #308).
- **Union graph**: The post-simulation graph built from just those causal edges whose recorded link score was ever active.
- **Activity bitset**: One bit per saved step per edge, recording whether that edge was active there. ANDing the bitsets along a path gives exactly the steps it can score, and an empty AND proves no extension of that path can score either.
- **The universe**: The complete set of cycles that could ever have a nonzero score, which exact enumeration produces within its budgets. Relative scores are fractions, so a correct denominator requires the whole population.
- **Retention / `MIN_CONTRIBUTION`**: A candidate loop is kept only if at some single timestep its absolute score reaches 0.1% of its partition's total score mass at that step.
- **Coverage-aware cap**: The rule deciding which loops make the 200-loop report under pressure: any loop that is the top loop at some timestep within a competing partition keeps its slot unconditionally, and remaining slots are filled in ranking order.
- **`enumeration_complete` / `universe_loops`**: Result fields saying whether the exact enumerator finished, and how many circuits the universe held.
- **Shortest-path fallback**: The second candidate generator that samples cycles when exact enumeration exhausts its budget or deadline. Deleted by this plan.
- **De-subscripting oracle**: A test harness that mechanically expands an arrayed model into the equivalent scalar model, simulates both, and compares values, link scores, loops and relative series. Reference section 15.4 makes the de-subscripted scalar model the correctness standard for arrayed LTM.
- **salsa**: The Rust incremental-computation framework the compiler is built on: inputs, memoized queries, query arguments that key separate memos, and revisions.
- **Overlay (`db::LtmOverlay`)**: Whether a compile assembles the LTM instrumentation, passed as an argument to every compile query rather than stored as an input on the project, so both variants stay memoized side by side.
- **Synthetic variable**: An auxiliary the LTM pass adds to the model to compute a score, named with a `$` prefix and the U+205A separator (`$⁚ltm⁚link_score⁚x→y`).
- **`Results`**: A run's output: a flat slab of every saved variable at every saved step, plus per-variable offsets naming the columns.
- **`ProbedDb`**: A test-only wrapper that records which tracked query bodies salsa actually executed over a measured region; used to prove absences (no re-execution, no second simulation).
- **Golden**: A retained expected output, captured from today's code before that code is deleted, against which the replacement is asserted.
- **World3**: The Limits to Growth world model; large and densely connected (a 166-node SCC), the stress case for loop enumeration.
- **C-LEARN**: Climate Interactive's climate policy model and the largest model in the repository; the compile and run performance benchmark of `docs/design/engine-performance.md`.

## Architecture

### The finding this plan acts on

The audit of 2026-09-07 established three facts that make the present two-mode design indefensible:

1. Both hero models (World3, C-LEARN) exceed `MAX_LTM_SCC_NODES = 50` and auto-flip to discovery, so exhaustive mode runs only on toy fixtures, and every consumer of the structural loop surface (`model_detected_loops`, libsimlin `get_loops`, pysimlin `Run.loops`, the CLI, the layout, TS `Model.loops()`) is empty on the models the project cares about. The TS engine has no discovery binding at all.
2. The post-simulation product of a loop's recorded link-score series reproduces the in-simulation `$⁚ltm⁚loop_score⁚` column to one ulp on all seven fixtures (38 loops, arrayed slots and cross-aggregate loops included). In-simulation loop scores carry no information the post-simulation product lacks.
3. All-edge instrumentation costs no more than loop-edge instrumentation plus loop-score variables on every corpus model (C-LEARN compile 0.80 vs 0.74 s, run 1.31 vs 1.33 s; toy models emit fewer variables under all-edge instrumentation because per-loop variables dominate).

Two further findings shape the plan: the exhaustive surface's relative normalization (per `(partition, slot)` bucket, `ltm_post::compute_rel_loop_scores_per_element`) is wrong on every coupled arrayed model while discovery's per-partition normalization is right; and static polarity is `Unknown` on 195 of World3's 200 and 150 of C-LEARN's 153 reported loops while runtime classification resolves every loop in the corpus at confidence 1.0.

### The pipeline

```
compile (LtmOverlay::On)
  model_causal_edges / model_element_causal_edges        which edges exist (unchanged)
  model_ltm_reference_sites, enumerate_agg_nodes           how each edge reads (unchanged)
  model_ltm_variables                                      one link score per element edge,
                                                           sub-model pathway/composite vars for
                                                           instances in a nontrivial SCC,
                                                           NO loop scores, NO mode
  model_structural_loops (Johnson, budgeted)               the pre-run loop list: element cycles,
                                                           canonical keys, ids; pins validated here
simulate
  the VM writes every link-score series into Results     (unchanged)
post-simulation (one function, every consumer)
  ltm_finding::score_loops(results, graph, structural, pins, budget)
    ActivityGraph::build -> enumerate_active_circuits      the universe (exact within budget)
    retain_circuits, materialize, pins injected            raw series per loop and slot
    ltm_post::relative_scores(per partition)               the one normalization
    LoopPolarity::from_runtime_scores(relative series)     polarity + confidence
    rank, coverage-aware cap, ids from structural keys     the report
```

`LtmOverlay` stays the compile key (`db::LtmOverlay`); there is no second key. What used to distinguish the modes (which edges get scores; whether loop scores are variables; whether Johnson's list or the post-simulation list is "the" loop list) collapses to: scores for all edges, loops from the post-simulation pipeline, Johnson for the structural preview and for ids.

### Loop identity and ids

A loop's identity is its element-level cycle: the canonical rotation of its element-subscripted node sequence (`ltm::canonical_cycle_key`, the one implementation that replaces `canonical_rotation`, `canonical_cycle_rotation`, `strip_element_subscript`, `dedup_trimmed_twins` and the rotation matching in `build_element_level_loops`). Two directed cycles over one node set are distinct keys (GH #308).

Ids are assigned from the structural list when Johnson completes within `MAX_LTM_CIRCUITS`: loops sorted by canonical key get `l1, l2, ...`, and a post-simulation loop whose key appears in the structural list takes that id. A post-simulation loop absent from the structural list (Johnson did not complete, or the loop runs through a reducer node the structural list also carries, so this is rare) gets an id from its key's position in the sorted reported set, after the structural ids. Pins keep `pin{n}` ids and carry the user's name. Ids therefore carry no polarity letter; the display label a UI shows (`R1`, `B2` by importance rank, the papers' convention) is a presentation concern computed from the run, not an identity.

This retires the exhaustive surface's A2A collapse (one `Loop` with `slot_links` and N slots): a loop is one element cycle, and an arrayed family is N loops sharing a variable-level shape. The FFI's subscripted access (`simlin_analyze_get_relative_loop_score("r1[nyc]")`, `simlin_analyze_get_loop_element_count`, pysimlin's `element=` accessors) goes with it; consumers that want the family group by the variable-level shape, which every reported loop exposes.

### Pins

`db/ltm/pinned.rs::model_pinned_loops` keeps its validation (it reads the causal graph, not a run) and its `pin{n}` ids, but emits no `loop_score` variable. The post-simulation pipeline injects each valid pin's element cycle(s) into the candidate set before retention, marks them exempt from retention and from the coverage-aware cap, and scores them with everyone else. This is the reference's `LOOPSCORE` semantics: the loop is reported regardless of what discovery found (reference section 10.2).

### Reducers

An inline reducer subexpression is already a node in the element graph (`$⁚ltm⁚agg⁚{n}`, `ltm_agg.rs`, `db/analysis.rs::emit_agg_routed_edges`). Today it is trimmed from reported loops and the loops that visit it more than once are reconstructed by petal stitching (`db/ltm/loops.rs::stitch_cross_agg_petals`), which emits one loop per petal subset. The audit showed this is neither the de-subscripted model's loop universe (which has `sum C(N,k) (k-1)!` circuits) nor the named-aggregate model's (N loops), and that its relative scores are off by construction. This plan adopts the named-aggregate semantics, which is also the papers' treatment of hidden structure (a macro's internal pathways collapse to the macro; reference sections 6.3 and 6.4): the reducer node is a real node, loops are the elementary circuits of the element graph, and the node is reported in the loop's sequence with the reducer's spelling (`SUM(pop)`) so a reader sees the loop close through the aggregate rather than reading `growth[a] -> pop[a]` as a self-loop. The stitcher, its budgets and its truncation flag are deleted.

### Sub-model instances

Pathway and composite variables exist so a loop through a module instance can be scored through the instance's strongest internal pathway (reference section 6.3) and, in the post-simulation pipeline, re-selected per exit port (`ltm_finding::recompute_module_input_edge_series`). An instance whose node is not in a nontrivial SCC of its parent's causal graph cannot lie on a loop, so no consumer ever reads its pathway scores; `model_ltm_variables` emits them only for instances in a nontrivial SCC (`causal_graph_from_edges(..).scc_of(node)`). The exhaustive-side override machinery (`db/ltm/mod.rs::compute_module_link_overrides`, `max_abs_alias_selection`, the `⁚via⁚` / `⁚viaacc⁚` alias variables) goes with the loop-score variables; the post-simulation recompute is the one owner.

### Polarity

`LoopPolarity::from_runtime_scores` classifies from a loop's partition-relative series (bounded, dominance-weighted; the audit's synthetic case shows raw sums let three inflection steps outweigh two hundred balancing steps). Before a run a loop has `polarity: None`. `ltm/polarity.rs` (static AST polarity), `CausalGraph::get_link_polarity` / `all_link_polarities` / `calculate_polarity`, `db::analysis::compute_link_polarities` and the agg-hop recovery in `db/ltm/loops.rs` are deleted. libsimlin `get_links` and the MCP `read_model` relationships report the runtime sign of each link's recorded series (same classifier, link series) when a run exists and `None` otherwise.

### Enumeration budgets

`enumerate_active_circuits` keeps its three budgets and the caller's deadline. When any trips, the partial circuit list is retained, ranked and reported with `enumeration_complete == false` and `universe_loops == None`. The audit measured the shortest-path fallback at recall 0.20 at the cap on World3 (the only corpus model dense enough to need it, where the exact enumerator finishes in 0.56 s) and 0.98 on C-LEARN (where the exact enumerator finishes in 47 ms); it never fires within default budgets on any model in the repository, so it is deleted rather than kept as a second generator.

### Surfaces

- libsimlin: `simlin_analyze_get_loops` (structural, pre-run), `simlin_analyze_get_loops_runtime` (post-run, the pipeline), `simlin_analyze_discover_loops` (renamed `simlin_analyze_loops`, the same pipeline over a sim's results), `simlin_analyze_loops_from_wasm_results` (the pipeline over a wasm slab, twin of `simlin_analyze_rel_loop_score_from_wasm_results`, which it replaces). `simlin_sim_get_ltm_mode`, `simlin_analyze_get_relative_loop_score`, `simlin_analyze_get_rel_loop_score`, `simlin_analyze_get_loop_score`, `simlin_analyze_get_loop_element_count` and the `SimState` partition/denominator caches are deleted; per-loop series come from the loops result.
- `@simlin/engine`: `Run.loops` populated on both engines through the new binding; `includeInternal` defaults to false; `Model.loops()` returns the structural list with `polarity: null`.
- pysimlin: `Run.loops` from the pipeline on every model; `LtmMode`, `Sim.get_loops_runtime`, the `element=` accessors and `Run._populate_loop_behavior`'s column reader are deleted; `Model.get_loops()` structural with `polarity None`.
- MCP: `analyze_model` calls the pipeline directly; `enumerationComplete` stays on the wire.
- CLI `--ltm`: prints the post-simulation loop report; `$⁚` columns are excluded from the TSV unless `--ltm-columns` is given.
- Layout: `layout::detect_ltm_loops` takes a `Results` (the display run's, when the caller has one) and calls the pipeline; it no longer runs its own simulation when results are supplied.

## Existing Patterns

- The post-simulation pipeline is discovery's existing `ltm_finding` module (`docs/design-plans/2026-08-17-ltm-discovery-exact.md`): union graph, activity bitsets, exact enumeration, retention against the universe, coverage-aware cap. This plan makes it the only pipeline; it changes nothing in its numerics.
- `db::LtmOverlay` as a query argument (`docs/design/engine-performance.md`, "The LTM overlay is an argument, not an input") is the pattern this plan extends by deleting the remaining mode input rather than converting it (GH #1056).
- One owner per compiler decision (`src/simlin-engine/CLAUDE.md`, "One owner") is the rule the consolidation phases enforce; the audit listed six LTM violations (partitions, dedup, normalization, runtime polarity readers, per-exit-port override, cross-agg stitching), each traceable to the mode split.
- The parity-against-a-retained-oracle pattern (`tests/integration/simulate_ltm_wasm.rs`) is reused: the seven fixtures' exhaustive `loop_score` series are captured as goldens before the exhaustive path is deleted, and the pipeline is asserted against them.
- The de-subscripting oracle built in the audit (`scratchpad/B/desub.py`, `oracle.py`) becomes an integration harness over `datamodel::Project`, following the CLAUDE.md rule that the only correctness standard for arrayed LTM is the de-subscripted scalar model (reference section 15.4).

Divergence from the design doc `docs/design/ltm--loops-that-matter.md`: its "Two Modes of Operation", "Cross-agg loop recovery", "Per-Slot Loop Score Equations", "Static Polarity" and "Passthrough composites and per-exit-port loop scoring" sections describe machinery this plan deletes; Phase 8 rewrites the document as if the code had always been single-mode.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Goldens and the one normalization
**Goal:** Capture the exhaustive path's loop-score series as goldens, and make per-partition normalization the single owner every reader uses.

**Components:**
- `tests/integration/ltm_single_mode_parity.rs` -- for each of the seven fixtures, the exhaustive `loop_score` series per loop and slot, captured once into `test/<fixture>/loop_scores.golden.tsv`, and the assertion that the post-simulation pipeline reproduces them to one ulp.
- `src/simlin-engine/src/ltm_post.rs` -- `relative_scores(per-partition)` as the one normalization; `compute_rel_loop_scores`, the per-element bucket grid, `LoopElementIndex` and the streaming readers deleted; `ltm_finding::signed_relative_scores` calls it.
- `src/libsimlin/src/analysis.rs`, `src/simlin-engine/src/layout/detect_ltm_loops.rs` -- read the owner.

**Dependencies:** Work package 1 item B1 (the per-partition rule) merged.

**Done when:** `ltm_single_mode_parity.rs` passes against the goldens through the discovery pipeline; `AC2.1`, `AC6.3`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: One instrumentation, loops from the pipeline
**Goal:** `model_ltm_variables` emits link scores for every edge and no loop scores; every consumer reads loops from the post-simulation pipeline; the mode is gone.

**Components:**
- `src/simlin-engine/src/db/ltm/mod.rs` -- the exhaustive branch, loop-score emission, `compute_module_link_overrides`, `max_abs_alias_selection`, `model_ltm_mode`, the auto-flip warnings and `LtmMode` deleted; pathway/composite emission kept (gated in Phase 6).
- `src/simlin-engine/src/ltm_augment.rs` -- `generate_loop_score_variables`, `generate_dimensioned_loop_score_equation`, `generate_link_product` and `resolve_link_score_name_for_loop` deleted.
- `src/simlin-engine/src/db/input.rs`, `db.rs`, `analysis.rs`, `wasmgen/module.rs` -- `ltm_discovery_mode` and its setters/threading deleted; `analyze_model` calls the pipeline.
- `src/simlin-engine/src/ltm_finding.rs` -- `score_loops(results, graph, structural, pins, budget)` as the one entry point; pin injection before retention (exempt from retention and cap).
- `src/simlin-engine/src/db/ltm/pinned.rs` -- validation kept; `expand_pin_on_element_graph` produces element cycles for injection; no `Loop` emission.
- `src/libsimlin/src/analysis.rs`, `simulation.rs`, `model.rs` -- `get_loops_runtime` and `discover_loops` on the pipeline; `simlin_sim_get_ltm_mode` and the per-loop score accessors deleted; `compile_to_wasm` loses its discovery flag.
- `src/pysimlin/simlin/{model,run,sim,analysis}.py` -- `Run.loops` from the pipeline; `LtmMode` and the column reader deleted.
- `src/simlin-mcp-core/src/tools/*` -- unchanged call shape, no flag.
- `src/simlin-cli/src/main.rs` -- the `--ltm` report from the pipeline.
- Tests: `simulate_ltm.rs`, `simulate_ltm_pinned.rs`, `db/ltm_unified_tests.rs`, `db/ltm_module_tests.rs`, `db/ltm_tests.rs` rewritten against `score_loops`' output; mode tests deleted.

**Dependencies:** Phase 1.

**Done when:** every LTM test is green against the pipeline; `AC1.1`-`AC1.4`, `AC2.2`, `AC2.4`, `AC4.1`-`AC4.4`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Loop identity, ids and the structural list
**Goal:** One canonical cycle key; ids from the structural list; the A2A collapse retired.

**Components:**
- `src/simlin-engine/src/ltm/mod.rs` -- `canonical_cycle_key` (element-level, direction-preserving) replacing `canonical_rotation`, `db::analysis::canonical_cycle_rotation` / `strip_element_subscript` / `strip_subscript`, `dedup_trimmed_twins`, `by_reported_cycle`, and `assign_pin_ids`' rotation match.
- `src/simlin-engine/src/db/analysis.rs` -- `model_structural_loops` (budgeted Johnson over the element graph, ids `l{n}` by key order) replacing `model_loop_circuits_tiered`, `classify_cycle`, `build_loops_from_tiered` and `model_detected_loops`' mode branch; `model_element_cycle_partitions` the one partition owner (`model_cycle_partitions` and `CausalGraph::compute_cycle_partitions` deleted).
- `src/simlin-engine/src/db/ltm/loops.rs` -- the A2A half of `build_element_level_loops` and `Loop::slot_links` deleted.
- `src/simlin-engine/src/ltm/types.rs` -- `Loop { key, id, nodes, links, stocks, partition, dimensions_shape }` without `slot_links`.
- Consumers of subscripted loop access (`src/libsimlin/src/analysis.rs` `parse_subscripted_loop_id`, `get_loop_element_count`; pysimlin `element=`) deleted; `DiscoveredLoop`/`LoopSummary` expose the variable-level shape for grouping.

**Dependencies:** Phase 2.

**Done when:** `AC6.1`, `AC6.2`, `AC8.2`; the seven fixtures report the element-level loop set the parity goldens name.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Delete the fallback
**Goal:** The exact enumerator is the only candidate generator; budget exhaustion reports a partial universe.

**Components:**
- `src/simlin-engine/src/ltm_finding_fallback.rs`, `ltm_finding_fallback_tests.rs`, `examples/ltm_fallback_eval.rs`, the fallback rows of `examples/ltm_discovery_bench.rs` -- deleted.
- `src/simlin-engine/src/ltm_finding.rs`, `ltm_finding_enum.rs` -- `enumerate_active_circuits` returns the partial list on a budget trip; `retain_circuits` and ranking run over it; `ENUM_BUDGET_FRACTION`, `FallbackConfig` and `fallback_candidates` deleted; `DiscoveryResult::truncated` folded into `enumeration_complete`.
- `docs/design/ltm--loops-that-matter.md` -- the fallback section replaced by the partial-universe rule.

**Dependencies:** Phase 2.

**Done when:** `AC5.1`, `AC5.2`; `rg -n "fallback" src/simlin-engine/src/ltm_finding*.rs` is empty.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: A reducer is a node
**Goal:** Loops through an inline reducer are the element graph's elementary circuits through the reducer node; the stitcher is gone.

**Components:**
- `src/simlin-engine/src/db/ltm/loops.rs` -- `stitch_cross_agg_petals`, `recover_cross_agg_loops`, `collect_agg_petals`, `MAX_AGG_PETALS`, `MAX_CROSS_AGG_LOOPS`, `AggLoopBudgetGuard`, `recover_agg_hop_polarities` deleted.
- `src/simlin-engine/src/ltm_finding.rs` -- `stitch_cross_agg_node_paths`, `trim_synthetic_aggs_from_loop_links` and `agg_recovery_truncated` deleted; the reported node sequence keeps the agg node, displayed by `AggNode::display_spelling` (the reducer's spelled text).
- `src/simlin-engine/src/ltm_agg.rs` -- `display_spelling` on `AggNode`.
- Surfaces: `agg_recovery_truncated` removed from `ModelAnalysis`, the FFI, pysimlin and MCP; loop node lists may contain an agg display node, flagged `synthetic: true` so a UI can style it.
- `tests/integration/ltm_array_agg.rs` -- cross-agg expectations rewritten to the named-aggregate semantics; `tests/integration/ltm_desubscript_oracle.rs` -- the audit's oracle over `datamodel::Project` (de-subscript, simulate both, compare values bit-exact, link scores, loops and relative series) on the arrayed fixtures and a generated corpus.

**Dependencies:** Phase 3.

**Done when:** `AC7.1`-`AC7.3`.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Static polarity deleted; module emission gated
**Goal:** Polarity is a runtime field; sub-model pathway/composite variables exist only for instances that can be on a loop.

**Components:**
- `src/simlin-engine/src/ltm/polarity.rs`, `polarity_tests.rs`, `with_lookup_tests.rs` -- deleted; `CausalGraph::get_link_polarity`, `all_link_polarities`, `calculate_polarity`, `db::analysis::compute_link_polarities` deleted; `Link` loses its static `polarity`.
- `src/simlin-engine/src/ltm/types.rs` -- `Loop::polarity: Option<LoopPolarity>`; `from_runtime_scores` over the relative series (the one classifier), applied to link series for `get_links` and the MCP relationships.
- `src/simlin-engine/src/db/ltm/mod.rs` -- pathway/composite emission gated on `causal_graph_from_edges(edges).scc_of(instance).len() > 1`.
- `src/simlin-engine/src/ltm/graph.rs` -- `all_links` and `find_loops_with_limit` deleted (no callers).
- pysimlin `test_ltm_polarity.py`, `engine/ltm/tests.rs` id assertions rewritten.

**Dependencies:** Phase 3.

**Done when:** `AC8.1`, `AC9.1`, `AC9.2`.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Every surface, and the layout on the display run
**Goal:** The TS engine, both wasm paths, pysimlin, the CLI and the layout all read the pipeline; no surface is empty on any model.

**Components:**
- `src/libsimlin/src/analysis.rs` -- `simlin_analyze_loops_from_wasm_results` replacing `simlin_analyze_rel_loop_score_from_wasm_results`; `simlin_analyze_links_from_wasm_results` keeps its role.
- `src/engine/src/{model,sim,run}.ts`, `src/engine/src/internal/{analysis,wasmgen}.ts`, `direct-backend.ts`, `worker-server.ts`, `worker-protocol.ts` -- `Run.loops` on both engines; `includeInternal` default false; `Model.loops()` structural.
- `src/simlin-engine/src/layout/detect_ltm_loops.rs`, `layout/mod.rs` -- accept a `Results`; `src/libsimlin/src/layout.rs`, `src/simlin-mcp-core/src/tools/edit_model.rs` -- pass the display run's results where one exists.
- `src/simlin-cli/src/main.rs` -- `$⁚` columns excluded unless `--ltm-columns`.
- `src/engine/tests/wasm-ltm.test.ts`, `src/pysimlin/tests/test_ltm.py`, `tests/integration/ltm_discovery_large_models.rs` -- the cross-surface assertions of `AC3`.

**Dependencies:** Phase 2 (pipeline), Phase 6 (polarity field shape).

**Done when:** `AC2.3`, `AC3.1`-`AC3.4`.
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Docs, ledger, epic
**Goal:** The documentation describes single-mode LTM as the only design; the cost ledger is recorded; the epic reflects what closed.

**Components:**
- `docs/design/ltm--loops-that-matter.md` -- rewritten: pipeline, identity and ids, pins, reducers, module gating, polarity, budgets, surfaces; mode, stitching, per-slot equations, static polarity and fallback sections removed.
- `docs/reference/ltm--loops-that-matter.md` -- sections 15.2, 16.10 and the Simlin implementation notes updated to the named-aggregate semantics.
- `src/simlin-engine/CLAUDE.md`, `src/libsimlin/CLAUDE.md`, `src/pysimlin/CLAUDE.md`, `src/engine/CLAUDE.md` -- surface changes.
- `docs/design-plans/2026-09-04-link-scores-from-fragments.md` -- its "What this plan does not change" list and its Phase 4 (no loop/pathway/composite text generators remain to convert) updated.
- This document's ledger section: pre- and post-plan compile, run, slots, discovery time for the corpus (`examples/ltm_full_bench.rs`).
- GH epic #488: clusters B, E, G updated; #1056, #760, #309 (resolution unchanged, noted), #677, #674 (pins carry direction now), #658, #755, #672, #665, #685, #701 closed or rescoped in the PR body.

**Dependencies:** Phases 1-7.

**Done when:** `AC10.1`; the docs contain no changelog sentences about the modes.
<!-- END_PHASE_8 -->

## Additional Considerations

**Owner decisions embedded in this plan** (each defaulted to the audit's recommendation; a different answer changes a phase, not the architecture):

- D1. Ids without a polarity letter (`l{n}`), polarity as a runtime field; display labels `R{n}`/`B{n}` are a UI concern. Alternative: keep `r/b/u` prefixes computed after the run, which makes ids unstable across parameter changes.
- D2. Element-level loops as the reported unit (an arrayed family is N loops sharing a shape). Alternative: keep the A2A collapse on the reported surface, which keeps `slot_links`, the subscripted FFI access and ~430 lines of the tiered enumerator.
- D3. Named-aggregate semantics for inline reducers (a reducer is a node, shown in the loop). Alternative: enumerate the de-subscripted model's `(k-1)!` orderings, which is what reference 15.4 literally promises and is super-exponential.
- D4. Static polarity deleted (pre-run loops carry no polarity). Alternative: keep a ten-line flow-to-stock sign rule for an unrun diagram; the audit found nothing else `polarity.rs` computes survives contact with a real model.
- D5. The CLI hides `$⁚` columns by default.

**Discovery cost on dense models.** World3's exact enumeration is 0.56 s per run; under always-on that is paid on every simulation. The wall-clock budget parameter of the pipeline exists; a product default for it, and whether a run should reuse the previous run's loop set when the model is unchanged, are decisions for the always-on product plan, not this one.

**Sampling resolution.** The pipeline scores loops at saved-step resolution (GH #309). The fragments plan's Phase 6 retains every-dt states natively, which is the input a per-dt pipeline needs; this plan does not change the resolution.

**Interaction with the fragments plan.** Independent and preferably first: the fragments plan's Phase 4 converts the loop-score, pathway and composite text generators to typed builders, and after this plan the loop-score generators and the exhaustive override aliases do not exist. Its "What this plan does not change" list (discovery mode, pinned loops, `ltm_discovery_mode`) needs rewording either way.

**What is not in scope.** The loop UI in `src/diagram`, the RK and conveyor policies for always-on, the equilibrium diagnostic (#504), and the per-dt sampling of #309 belong to the always-on product plan that follows this one.
