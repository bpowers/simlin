# LTM discovery: exact enumeration as the sole primary path, shortest-path fallback

## Summary

Discovery-mode LTM finds the loops that matter after a simulation by (1) generating
candidate cycles from the recorded link-score series, (2) scoring each candidate exactly
(the per-step product of its link scores), and (3) retaining, ranking, and capping the
result. Only stage 1 is lossy. The 2026-08-10 plan made a union-graph circuit enumeration
the primary generator, with the paper's per-step strongest-first DFS kept as a fallback.
This plan finishes that transition and hardens it:

- The enumeration is the **only exact generator**. Its output is the full universe of
  ever-simultaneously-active elementary cycles (at saved-step resolution), never
  including single-variable cycles, and every downstream statistic (retention
  denominators, competing-vs-solo classification, relative scores) is computed against
  that universe.
- The per-step DFS -- a per-node-capped sampler that saturated silently on World3 --
  is **deleted**. Its role (bounded work when enumeration is infeasible; partial results
  under a wall-clock budget) is taken by a **shortest-path fallback**: for every
  (stock, saved step), a Dijkstra search over the step's active edges weighted by
  `w = -log|link score|` recovers the strongest cycle(s) through that stock. The weight
  function is pluggable and the choice is settled empirically against the exact
  enumeration on World3, not by argument.
- The enumeration pipeline is made **cheap** (World3 end-to-end under 1 s in release,
  from 5.2 s), **robust** (NaN/Inf-safe totals, deadline-aware at every phase, memory
  bounded at the budget), and **coverage-aware** at the cap (each step's dominant loops
  are guaranteed a slot).
- Completeness becomes **observable** end to end: `enumeration_complete` reaches
  libsimlin, pysimlin, and the MCP tools, and the audits that found the original defect
  are regenerable scripts that re-run against the new code.

## Definition of Done

1. Union-graph enumeration is the ONLY exact candidate generator: the per-step DFS
   (`SearchGraph` oracle, `IndexedSearch` DFS, `DfsScratch`, expansion cap + guard,
   `CandidateGen::DfsOnly`, `expansion_cap_saturated`) is deleted. Single-variable
   cycles are never emitted or reported; stockless multi-node cycles are kept (Solo
   group) and re-evaluated after re-measurement.
2. A shortest-path fallback replaces the DFS: per (stock, step) Dijkstra over
   `w = -log|link score|` (starting formulation; weight function pluggable),
   deadline-checkable, used when enumeration budgets/deadline trip. The wall-clock
   budget contract (partial results + `truncated`) is preserved and re-tested
   independently of the DFS.
3. Performance: full World3 discovery under 1 s (release) via all the identified
   improvements (contiguous per-edge series, single-pass retention bound, edge-row
   emission, no per-visit allocation, hoisted maps, memoized module recompute), with
   memory bounded at the enumeration budget.
4. Robustness fixes: NaN/Inf products in retention totals, module-override mass in
   denominators, reported-cycle dedup vs totals, budget split between enumeration and
   fallback.
5. Coverage-aware selection under `MAX_LOOPS` (each step's top-k by relative score
   guaranteed a slot, then fill by mean).
6. `enumeration_complete` (and therefore fallback-used) exposed through libsimlin,
   pysimlin, and MCP read/edit outputs.
7. Evidence: generator scripts (notebooks/ convention, outputs gitignored) re-run the
   C-LEARN and World3 audits against the new code; a comparison harness measures
   fallback weight formulations' recall against the exact enumeration on World3 with the
   enumerator forced off; docs (engine CLAUDE.md, docs/design/ltm doc, docs/README,
   tech-debt #24/#28) rewritten evergreen.

Out of scope: unifying compile-time exhaustive mode with post-sim discovery; a
TypeScript/WASM discovery surface; changing link/loop score definitions.

## Acceptance Criteria

Prefix: `ltm-discovery-exact`. Each criterion names the observable behaviour and, where
useful, the failure case that must NOT happen.

- **AC1.1** On any model, no reported discovery loop has a single link (`links.len() >= 2`),
  and the enumerator never emits a length-1 circuit even when a node has an active
  self-edge (the PREVIOUS-latch fixture reports only the population loop).
- **AC1.2** `CandidateGen::DfsOnly`, `DiscoveryResult::expansion_cap_saturated`,
  `ModelAnalysis::expansion_cap_saturated`, `SearchGraph`, `DfsScratch`,
  `EXPANSION_BUDGET_PER_SEARCH`, `dfs_expansion_budget`, `DfsExpansionBudgetGuard`,
  `IndexedSearch::{load_step_scores,discover_step,dfs,record_loop}` no longer exist.
  `rg` over `src/` and `docs/` (excluding `docs/reference/`, which transcribes the
  external papers, and `docs/design-plans/`, which are historical) finds no reference
  to the strongest-path DFS as current implementation.
- **AC1.3** A 2+-node cycle with no stock (module-level state or a PREVIOUS lag across
  auxes) is still reported, in a Solo normalization group, ranked after competing loops.
- **AC2.1** With the enumerator disabled (`CandidateGen::FallbackOnly(_)` or an
  `EnumBudgetGuard` that trips), the logistic fixture reports the same two loops with
  identical per-step scores as the enumeration path.
- **AC2.2** With a deadline that expires during enumeration or retention, discovery
  returns `truncated == true`, `enumeration_complete == false`, and a NON-EMPTY loop
  list whenever the fallback processed at least one saved step before the deadline
  (partial results, not nothing). The libsimlin `discover_loops_tiny_budget_truncates`
  and pysimlin `test_tiny_timeout_truncates` tests pass without relying on any deleted
  DFS behaviour, and a new engine test pins that an already-expired deadline is caught
  inside `ActivityGraph::build`, `enumerate_active_circuits`, `retain_circuits`, and the
  fallback sweep respectively (one arm per phase).
- **AC2.3** The fallback's weight function is selectable (`FallbackWeight` enum with at
  least `ClampedLogAbs`, `RelativeLinkScore`, `HopCount`), Dijkstra is exact for
  non-negative weights (unit test against brute-force shortest cycle on small graphs),
  every emitted cycle is elementary and closes through the seed stock, and the fallback
  never emits a cycle with a zero/NaN link at the step it was found.
- **AC2.4** The fallback finds every loop the enumeration finds on the tests' small
  fixtures (logistic, diamond, cross-agg, module) -- discovery's semantic tests are
  parametrized over `Auto` and `FallbackOnly(default weight)`.
- **AC3.1** `examples/ltm_discovery_bench` (release) reports World3 total discovery time
  under 1.0 s and C-LEARN under 0.2 s, with `enumeration_complete == true` on both;
  the numbers are recorded in this document's "Measured" section.
- **AC3.2** Retention survivors and their scores are bit-identical before and after the
  performance work (pinned by a fixture-level test on a model with NaN and zero links,
  and by the World3 audit's survivor count).
- **AC3.3** Enumeration memory is bounded by an explicit circuit-edge budget
  (`MAX_DISCOVERY_ENUM_EDGE_ROWS`) rather than only a circuit count; a test with a tiny
  override trips it and falls back.
- **AC4.1** A circuit whose product overflows to Inf and then meets a 0 link (NaN
  product with no NaN link) contributes nothing to its partition's totals and cannot
  satisfy retention; the partition's other loops keep finite relative scores at that
  step (regression test with hand-built series through `retain_circuits`).
- **AC4.2** For a module-traversing loop, the mass added to its partition's denominator
  is the per-exit-port override series (the score the loop reports), not the raw
  composite product; a fixture where the two differ pins the denominator.
- **AC4.3** When two enumerated circuits trim to the same reported loop, only the kept
  representative's mass remains in the partition totals.
- **AC4.4** With a wall-clock budget, enumeration + retention consume at most a fixed
  fraction of the budget (documented constant) before yielding to the fallback, so the
  fallback always has time to process at least the first steps.
- **AC5.1** Under `MAX_LOOPS` pressure, for every saved step and every competing
  partition, the loop with the largest |relative score| at that step among retention
  survivors is in the reported set (k >= 1 guaranteed); the reported list order remains
  the competitive-first mean-relative ranking; a fixture with a briefly-dominant loop
  and a tiny `MaxLoopsGuard` pins it, and World3's "step-dominant loop absent" count is
  0 in the regenerated audit.
- **AC5.2** Competing-vs-solo classification on the enumeration path uses the universe
  (a partition with >= 2 ever-active circuits is competing even if only one survives
  retention); a loop alone in its partition's universe is Solo; a fixture pins each arm.
- **AC6.1** `SimlinDiscoveryResult` (C header regenerated), pysimlin `Analysis`, and MCP
  `ReadModelOutput`/`EditModelOutput` carry `enumeration_complete`, populated from the
  engine value, with a test per layer mirroring the existing `agg_recovery_truncated`
  tests.
- **AC7.1** `notebooks/build_ltm_discovery_audit.py` (parametrized by model) regenerates
  the C-LEARN and World3 audit notebooks under `notebooks/` (gitignored), including a
  pure-Python union-graph enumeration cross-check; its verify script passes against the
  new engine; `docs/audits/` no longer exists and nothing references it.
- **AC7.2** `examples/ltm_fallback_eval` forces the fallback on World3 for each
  `FallbackWeight` and reports recall of the exact top-K (K = 10, 50, 200) and
  step-dominant coverage; the table is recorded in this document and the default weight
  is the measured best.
- **AC7.3** `src/simlin-engine/CLAUDE.md`, `docs/design/ltm--loops-that-matter.md`
  ("Strongest-Path Algorithm" section replaced), `docs/README.md`, and
  `docs/tech-debt.md` (#24, #28 closed) describe the current design with no changelog
  sentences.

## Glossary

- **Union graph**: the discovery edge set restricted to edges whose |link score| is
  finite and nonzero at >= 1 saved step in `1..step_count`.
- **Activity bitset**: per-edge bitset over saved steps marking where the edge is active.
  The running AND along a path is nonempty iff the path is simultaneously active at
  some step.
- **Universe**: the set of elementary cycles of the union graph whose activity AND is
  nonempty -- exactly the loops that can ever have a nonzero score.
- **Retention**: keeping a loop iff at some step its |score| is >= `MIN_CONTRIBUTION`
  (0.1%) of its partition's total |score| mass at that step.
- **Fallback**: the shortest-path candidate generator used when the enumeration cannot
  complete within its budgets or deadline.
- **Seed stock**: the stock a fallback Dijkstra starts from; every SD feedback loop
  contains a stock (module-level and PREVIOUS state excepted).

## Architecture

### Pipeline (unchanged shape, new internals)

```
parse_link_offsets  ->  IndexedSearch::build (topology, node ids)
      |
      v
ActivityGraph::build          contiguous per-edge score rows + activity bitsets + union SCCs
      |
      +--> enumerate_active_circuits   (Tiernan min-root + per-root induced-SCC + bitset AND)
      |         | complete?  --yes-->  retain_circuits (single-pass bound + confirm)  --> survivors, universe totals
      |         `-- no (budget/deadline) --> discard, fall back
      |
      +--> fallback::sweep              per step: active adjacency; per seed stock: Dijkstra; emit closing cycles
                                        deadline-checked; partial steps => truncated
      v
materialize FoundLoop (unchanged: links, module override series, trim, dedup)
      v
rank_and_filter (universe denominators; universe competing; coverage-aware cap; ids)
```

### Enumerator (`ltm_finding_enum.rs`)

- Emits circuits as **edge-row sequences** (`Vec<u32>` of `ActivityGraph` edge rows,
  closing edge included). Node paths are derived (`from` of each row) only where a
  consumer needs them (stitching, materialization).
- **No singleton circuits**: the closing test `to == root` is skipped at depth 1.
  An elementary cycle never repeats a node, so a self-edge can never be part of a
  longer cycle; self-edges are therefore never traversed at all and can be dropped
  from the union graph at build time. A one-variable "loop" is not a feedback loop in
  the SD sense (the exhaustive enumerator's `circuit.len() > 1` contract), and both
  surfaces must agree on what a loop is.
- **Per-root induced-subgraph SCC**: for root `r`, only nodes in the SCC of `r` within
  the subgraph induced by nodes `>= r` (Johnson's `A_k`) are explorable. This is exact
  (any cycle rooted at `r` lives entirely in that SCC) and removes the dead-end
  wandering that made 2/3 of World3's 20M descents fruitless.
- **No per-visit heap allocation**: the running AND is written straight into
  `and_stack` and truncated on prune, for any bitset width.
- **Budgets**: circuit count AND total edge rows (`MAX_DISCOVERY_ENUM_EDGE_ROWS`),
  visit budget, deadline (checked every `DEADLINE_CHECK_INTERVAL` visits). Any trip
  returns `complete: false` and the partial list is discarded (a partial enumeration is
  root-order-biased and its totals are not the universe; the fallback is the principled
  sample).
- Each circuit also carries its activity AND bitset (from the emission point), which
  lets scoring skip provably-zero steps.

### Activity graph and scoring

- `ActivityGraph` owns `series: Vec<f64>` -- for each union edge row, a contiguous
  slice of its signed score at every saved step (`step_count` floats), copied once from
  the results slab. All scoring multiplies edge-outer/step-inner over these rows;
  nothing reads `results.data` at stride during retention.
- `score_series(rows, out, nan_mask)`: `out` starts at 1.0, multiplies each row in;
  `nan_mask[t] = out[t].is_nan()` afterwards -- so an Inf*0 product is treated exactly
  like a NaN link (excluded from totals, cannot satisfy retention). Materialization keeps
  its own rules unchanged.
- Optionally restrict scoring to the circuit's active steps (bits set); inactive steps
  are exactly 0 or NaN and contribute no mass either way. Whether this pays for itself
  is measured, not assumed.

### Retention (`retain_circuits`)

Single streaming pass with a confirm step:

1. Pass 1 (all circuits): compute the series, add |s| into the partition's running total,
   and record `bound_i = max_t |s_i(t)| / running_total_i(t)` (the running total is a
   lower bound of the final total, so `bound_i >= true peak ratio`). Also count circuits
   per partition (universe competing classification) and mark module-traversing and
   Solo circuits.
2. Confirm (only circuits with `bound_i >= MIN_CONTRIBUTION`, plus module and Solo
   circuits): recompute the exact ratio against the final totals for the non-trivial
   arms. Solo circuits are ever-active by construction and pass; module circuits are kept
   unconditionally but contribute NO raw mass in pass 1 -- their reported (override)
   mass is added after materialization.
3. Output: survivors, universe partition totals (raw-mass), universe per-partition
   circuit counts.

### Fallback (`ltm_finding_fallback.rs`)

- Per saved step `t in 1..step_count`: build the step's active adjacency from
  `IndexedSearch` (edges with finite nonzero |score|; Inf edges have weight 0 under the
  clamped formulation), compute per-step SCCs, and for each seed stock inside a
  non-trivial SCC run Dijkstra restricted to that SCC.
- Weight function `FallbackWeight`:
  - `ClampedLogAbs`: `w = max(0, -ln|s|)` -- the user's starting point; sub-unit links
    cost, super-unit links are free (an admissible optimistic bound).
  - `RelativeLinkScore`: `w = -ln(|s| / sum_{x->z}|s_x|)` -- the LTM "relative link
    score" (reference doc 13.3), always >= 0; per-target normalization.
  - `HopCount`: `w = 1` -- SILS-style control.
- Cycle recovery: after Dijkstra from seed `s`, every in-edge `(u -> s)` with `u`
  reached closes a simple cycle `path(s..u) + (u -> s)`. All such cycles are emitted
  (deg_in(stock) is small); the minimum-weight one is the step's "strongest through s".
  Optionally, `k`-best via re-running with the best cycle's closing edge removed
  (bounded, off by default; the harness decides).
- Dedup by canonical rotation across steps and stocks; emitted node paths flow into the
  unchanged materialization/stitching pipeline. Deadline checked between Dijkstras;
  expiry mid-sweep keeps everything found so far and sets `truncated`.
- Cost: `T * S * E log V` -- World3 ~401 * 15 * 430 log 258 ~ 2e7 heap operations.

### Budget split

With a caller budget `B`: enumeration (`ActivityGraph::build` included) and retention
may spend at most `ENUM_BUDGET_FRACTION * B` (0.5); if they have not completed by then,
the fallback runs with the remainder. Every phase reads the clock at bounded intervals
(`ActivityGraph::build` per edge batch, the enumerator per visit batch, retention per
circuit batch, the fallback per Dijkstra). An unbudgeted call never reads the clock.

### Ranking (`rank_and_filter`)

- Universe denominators (external totals) as today, corrected for module override mass
  and reported-cycle dedup.
- Competing classification: enumeration path -> a partition is competing iff its
  universe circuit count >= 2; fallback path -> over the discovered set (as today).
- Coverage-aware cap: after retention, mark as **anchored** every loop that is, at some
  step, the |relative-score| maximum within a competing partition (k = 1). If the
  anchored set fits, fill remaining slots by the existing competitive-first mean-relative
  order; if it does not fit (pathological), anchors are ranked by mean-relative and the
  cap applies to them alone. Raise k while the anchored set still fits (bounded by a
  small constant, e.g. 3). Presentation order is unchanged (competitive-first,
  mean-relative, magnitude tie-break, content key), only membership changes; ids stay
  content-derived.

### Observability

`DiscoveryResult { loops, partitions, truncated, agg_recovery_truncated,
enumeration_complete }`. `enumeration_complete == false` means the fallback generated
the candidates (a sample). Threaded to `ModelAnalysis`, `SimlinDiscoveryResult` (+
`simlin.h`), pysimlin `Analysis`, MCP `ReadModelOutput`/`EditModelOutput` (always
serialized, wire name `enumerationComplete`).

## Existing Patterns Followed

- Test-only budget overrides via RAII guards + thread_local (`MaxLoopsGuard`,
  `EnumBudgetGuard`) rather than large fixtures (docs/dev/rust.md test-time budgets).
- Flag threading pattern from `agg_recovery_truncated`: field + doc + copy site + one
  test per layer (libsimlin `tests/integration/analysis.rs`, pysimlin
  `tests/test_discovery.py`, mcp-core `read_model_e2e.rs`).
- Notebook generators: `notebooks/build_*.py` (nbformat + nbclient) with a
  `verify_*.py` that fails on error outputs, silent cells, missing figures, and missing
  claim markers; interpreter `src/pysimlin/.venv/bin/python`; outputs gitignored under
  `/notebooks/`.
- Examples as measurement instruments (`examples/ltm_discovery_bench.rs`,
  `examples/ltm_search_graph_dump.rs`) built on public engine entry points, never
  re-deriving the edge set.

## Implementation Phases

Each phase ends with a review (code-reviewer agent) and a green pre-commit run. Phases
1-2 are sequential (both edit `ltm_finding*.rs`); 3 and 5 can run in parallel with 4;
6 is last.

### Phase 1 -- Enumeration correctness and performance

Enumerator: no singletons, edge-row emission, per-root induced SCC, no per-visit
allocation, edge-row budget, activity AND carried per circuit. Activity graph:
contiguous series. Retention: single pass + confirm, NaN/Inf-safe, module circuits
excluded from raw mass, universe partition counts. Materialization: hoisted edge lookup,
memoized module recompute keyed by (module, entry, exit), module mass added to totals
after materialization, dedup mass subtraction. Bench: World3 < 1 s. Tests: AC1.1, AC3.1,
AC3.2, AC3.3, AC4.1, AC4.2, AC4.3.

### Phase 2 -- Shortest-path fallback, DFS deletion, budget contract

New `ltm_finding_fallback.rs` (weights, Dijkstra, sweep, deadline). Wire as the fallback
with the budget split; add deadline checks to `ActivityGraph::build`. Delete every DFS
item and its tests; port the semantic tests to run over `Auto` and `FallbackOnly`;
re-implement the deadline tests per phase; keep `discovery_graph_stats` (generator-
independent diagnostic) but reword its docs. `CandidateGen::{Auto, FallbackOnly(FallbackWeight)}`.
Tests: AC1.2, AC1.3, AC2.1-AC2.4, AC4.4.

### Phase 3 -- Fallback evaluation harness and audit generators

`examples/ltm_fallback_eval.rs` (World3 + C-LEARN, each weight, recall of exact
top-K and step-dominant coverage, wall-clock). `notebooks/build_ltm_discovery_audit.py`
+ `verify_ltm_discovery_audit.py`, parametrized by model, consuming
`ltm_search_graph_dump`; delete `docs/audits/`. Run both; pick the default weight;
record the tables in this document. Tests: AC7.1, AC7.2.

### Phase 4 -- Selection

Universe-based competing classification; coverage-aware cap; solo magnitude tie-break
kept. Re-run the World3 audit for the step-dominant-absent count. Tests: AC5.1, AC5.2.

### Phase 5 -- Surface `enumeration_complete`

libsimlin struct + header + test; pysimlin dataclass + analyze() + test; mcp-core
outputs + e2e tests. Tests: AC6.1.

### Phase 6 -- Documentation, tech debt, re-evaluation

Rewrite the engine CLAUDE.md paragraph and `docs/design/ltm--loops-that-matter.md`
discovery section; update docs/README; close tech-debt #24/#28; record measured numbers
here; re-examine stockless multi-node loops on C-LEARN/World3 output and either keep
with rationale or propose a change. Tests: AC7.3.

## Additional Considerations

- **Why discard a partial enumeration rather than merge it with the fallback**: the
  partial set is biased by node-id root order, cannot supply universe denominators, and
  materializing it risks the multi-GB cliff. Simplicity wins; if measurement later shows
  merged recall matters, it is an additive change.
- **Why not Johnson potentials for exactness**: a negative cycle in `-log` space is a
  loop with gain > 1; World3 has ~1/3 of active links super-unit at typical steps and
  loops with |score| ~1e10, so feasible potentials do not exist. Clamping keeps
  Dijkstra's preconditions; the relative-link-score weighting is the principled
  always-non-negative alternative. The harness arbitrates.
- **Sub-save-step activity** remains invisible to both generators (shared with the
  literature's per-step method); documented, not fixed here.
- **Determinism**: enumeration order is content-pure (node ids from insertion order of
  parsed offsets, which is sorted); the fallback's emitted set is order-independent
  after canonical-rotation dedup; ranking ties break on content keys.

## Measured

Baseline (this branch before Phase 1, release, Apple M-series under Asahi):

| Model | Enumeration total | of which enumerate / retain | Circuits | Survivors | Reported |
|---|---|---|---|---|---|
| C-LEARN v77 | 0.037 s | -- | 211 | -- | 200 (48 singletons) |
| World3-03 | 5.2 s | 0.37 s / 4.75 s | 150,827 | 2,979 | 200 |

Reported-200 share of universe partition mass per step on World3: min 0.006, median
0.59, max 0.92. Super-unit active links per step: World3 37-91 of ~190-250; C-LEARN
288-602 of ~1300-2400.

After Phase 1 (release, Apple M-series under Asahi, `examples/ltm_discovery_bench`):

| Model | Enumeration total | Circuits | Survivors | Reported | `enumeration_complete` |
|---|---|---|---|---|---|
| C-LEARN v77 | 0.036 s | 162 | 153 | 153 | true |
| World3-03 | 0.40 s | 150,827 | 2,979 | 200 | true |

World3 phase breakdown at 0.40 s: activity-graph build 0.4 ms, enumerate 143 ms,
retain 137 ms, cross-agg stitch 0.4 ms, `FoundLoop` materialization + dedup 87 ms,
rank 28 ms, remainder (`parse_link_offsets`, `IndexedSearch::build`, cycle
partitions) ~5 ms. C-LEARN at 36 ms is dominated by the phases *before* candidate
generation: 6 ms activity-graph build over 3,014 union edges, ~27 ms
`parse_link_offsets` + topology build over its ~26k LTM variables, and under 2.5 ms
in enumeration, retention, materialization and ranking combined.

C-LEARN's reported count falls from 200 to 153 because 48 of the old 200 were
single-variable `PREVIOUS`-latch self-loops, which are no longer loops (AC1.1); the
`MAX_LOOPS` cap no longer binds there.

World3's universe is ~150k circuits, not the ~330k quoted in earlier notes: 330k is
its elementary-cycle count WITHOUT the ever-simultaneously-active constraint, which
the enumerator never materializes.

After Phase 2 (release, Apple M-series under Asahi,
`examples/ltm_discovery_bench`, both generators over the same simulated
results):

| Model | Generator | Discovery time | Loops | `enumeration_complete` |
|---|---|---|---|---|
| C-LEARN v77 | `Auto` (enumeration) | 0.038 s | 153 | true |
| C-LEARN v77 | fallback `ClampedLogAbs` | 0.072 s | 97 | false |
| C-LEARN v77 | fallback `RelativeLinkScore` | 0.068 s | 97 | false |
| C-LEARN v77 | fallback `HopCount` | 0.065 s | 72 | false |
| World3-03 | `Auto` (enumeration) | 0.409 s | 200 | true |
| World3-03 | fallback `ClampedLogAbs` | 0.020 s | 59 | false |
| World3-03 | fallback `RelativeLinkScore` | 0.020 s | 48 | false |
| World3-03 | fallback `HopCount` | 0.010 s | 23 | false |

AC3.1 holds (World3 0.409 s < 1.0 s, C-LEARN 0.038 s < 0.2 s, both complete).
The loop COUNTS above are not yet a recall measurement: a fallback run reports
fewer loops partly because it proposes fewer candidates and partly because its
retention denominators are its own discovered set rather than the universe.
Phase 3's harness measures recall of the exact top-K, which is what settles
the default weight; the fallback columns here establish only that every
formulation is fast enough to be a usable fallback on both models.

(Phases 3, 4 and 6 still to fill in.)
