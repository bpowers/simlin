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
  `rg` over `src/` finds no reference to the strongest-path DFS as current
  implementation; the `docs/` sweep (excluding `docs/reference/`, which transcribes the
  external papers, and `docs/design-plans/`, which are historical) is Phase 6's (AC7.3).
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
  fixtures whose cycles are distinguishable by a shortest-path tree (logistic, cross-agg,
  module); discovery's semantic tests are parametrized over `Auto` and
  `FallbackOnly(default configuration)`. Where the fallback structurally differs, the
  difference is pinned with its mechanism. Both anticipated differences were closed by
  the Phase 3b measurements and are pinned in their new direction: a diamond's two arms
  are both recovered (the every-edge closure family reaches the arm no single tree
  expresses), and a stockless cycle is reached by the default seed policy while staying
  unreachable from stock seeds alone (AC1.3) -- each arm pinned separately, so what the
  wider setting buys is stated rather than assumed.
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

The shipped design (`FallbackConfig { weight, seeds, closures, tie_break }`, default
`ClampedLogAbs` / `StocksAndStocklessSccs` / `EveryEdge` / `Hops` -- settled by the
sweeps in "Measured" below, not by the argument that motivated the starting point):

- Per saved step `t in 1..step_count`: build the step's active adjacency from
  `IndexedSearch` (edges with finite nonzero |score|; an Inf edge weighs 0 under every
  arm), compute per-step SCCs, and for each seed run Dijkstra restricted to its SCC.
- Seeds (`FallbackSeeds`): `Stocks` (every SD feedback loop contains one); the default
  `StocksAndStocklessSccs` additionally seeds one representative per non-trivial
  stockless SCC, closing AC1.3's gap (a cycle whose state hides in a module level or a
  `PREVIOUS` lag between auxes); `AllSccNodes` seeds the whole cyclic core and stays
  selectable but unused -- measured slower per unit of recall than the default.
- Weight function `FallbackWeight` -- every arm must be non-negative because Dijkstra's
  optimality proof needs it, and a super-unit link (gain > 1) is a NEGATIVE edge in raw
  `-ln` space with no feasible Johnson potentials, so each arm handles it differently:
  - `ClampedLogAbs` (default): `w = max(0, -ln|s|)`. The clamp is an UPPER bound on the
    true `-ln` cost, not a lower one: it DISCARDS a super-unit link's gain rather than
    expressing it, making every such hop free and creating a zero-weight plateau (a
    third of a real graph's active links, on these models) that the tie-break resolves.
  - `RelativeLinkScore`: `w = -ln(|s| / sum_{x->z}|s_x|)`, the LTM "relative link score"
    (reference doc 13.3); a share is at most 1, so it is non-negative without clamping.
  - `HopCount`: `w = 1` -- the score-blind control every other arm has to beat.
  - `ShiftedLogAbs`: `w = ln(step max finite |s|) - ln|s|`, keeping the gain the clamp
    discards by shifting instead of clamping. Measured and REJECTED (see "Measured"):
    on these models the per-hop shift dwarfs the product term and the arm degenerates
    into a hop count. Stays selectable as a documented negative result.
- Tie-break (`FallbackTieBreak`, default `Hops`): orders two routes of equal weight.
  `Hops` (fewer first) is a statement about the model -- among equally-weighted routes
  under the clamp's zero-weight plateau, the shorter one is the loop a modeller means;
  `NodeId` is the measurement control. Both search orderings sort on `(weight, hops)` so
  the tie-break composes with every weight arm.
- Closures (`FallbackClosures`) -- which cycles a completed search closes:
  - `SeedInEdges`: only the seed's own in-edges close, giving the minimum-weight
    elementary cycle through the seed. One forward Dijkstra per (seed, step); cheap and
    narrow, since one shortest-path tree holds one route per node.
  - `EveryEdge` (default): a REVERSE Dijkstra also runs to the seed, and every edge
    `u -> w` inside the seed's SCC whose source the forward tree reached and whose
    target the reverse tree reached closes `path(seed..u) + (u -> w) + path(w..seed)` --
    the minimum-weight cycle through BOTH the seed and that edge, the strength-weighted
    analogue of edge coverage. A closure whose two tree halves share a node is not
    elementary and is SKIPPED rather than spliced (a spliced walk is no longer the
    minimum through its edge). This is the lever that earns its cost: it is what raises
    recall from a small fraction of the exact top-K to the numbers in "Measured".
- Dedup: emitted cycles are deduped by a rotation-independent fingerprint over the
  cycle's directed edge set (a bucket hit is resolved by an exact rotation comparison),
  so the dedup is exact and free for the common case (after the first few saved steps
  nearly every candidate is a duplicate).
- Candidate bound: `MAX_FALLBACK_PATHS` (200,000, checked at every dedup insert) caps
  how many distinct cycles one sweep may accumulate; a trip stops the sweep and reports
  `truncated`, the same signal a deadline expiry gives, since both mean the sweep did
  not get to sample everything it would have.
- Deadline sites: the clock is read at three bounded places -- once at the top of each
  step, once before each seed's searches, and once per fixed pop interval inside a
  search (so one seed whose component is most of the graph cannot overrun the budget on
  its own). An unbudgeted sweep reads the clock nowhere.
- What the sweep drops is CHARACTERIZABLE, not an artifact of sampling density: a cycle
  is reachable iff at some step, for some seed on it and some edge on it, it is the
  MINIMUM-WEIGHT cycle through both -- so the recall ceiling is an optimality
  restriction (which cycle wins the competition for a given seed/edge/step), not a
  question of how much of the graph got visited. On the `ClampedLogAbs` zero-weight
  plateau many cycles tie at the minimum for a given (seed, edge); the sweep emits ONE
  per pair (the tie-break's choice) rather than the whole tied set, which is the
  unmeasured lever a future k-best-under-ties extension would pull.
- Cost: `steps * seeds * E log V` -- World3 ~401 * 15 * 430 log 258 ~ 2e7 heap
  operations under `SeedInEdges`; `EveryEdge` adds a second search plus one path check
  per edge per (seed, step).

### Budget split

With a caller budget `B`: enumeration (`ActivityGraph::build` included) and retention
may spend at most `ENUM_BUDGET_FRACTION * B` (0.5); if they have not completed by then,
the fallback runs with the remainder. Every phase reads the clock at bounded intervals
(`ActivityGraph::build` per edge batch, the enumerator per visit batch, retention per
circuit batch, the fallback per Dijkstra). An unbudgeted call never reads the clock.

### Ranking (`rank_and_filter`)

- Universe denominators: `rank_and_filter` takes `universe: Option<&UniverseStats>` --
  `Some` on the enumeration path (the full enumerated universe's per-partition raw mass
  and circuit counts), `None` on the fallback path (a sample has no universe, so the
  discovered set is measured against itself). Corrected for module override mass and
  reported-cycle dedup.
- Competing classification: on the enumeration path a partition is competing iff its
  UNIVERSE circuit count (`UniverseStats.loop_counts`) is >= 2 -- a retention
  non-survivor still holds mass in the denominator, so it still makes the partition a
  place where loops compete; on the fallback path, over the discovered set, since a
  sample has no universe to ask about.
- Coverage-aware cap (`select_reported`): after retention, mark as **anchored** every
  loop that is, at some step, the |relative-score| maximum within a competing partition
  (`k = 1`, unconditional -- this is the AC5.1 guarantee and is exempt from the bound
  below). If the k=1 anchor set alone overflows the cap (pathological), the cap applies
  to the anchors alone, ranked by mean-relative. Otherwise `k` may rise, bounded by
  `MAX_ANCHOR_K`, but ONLY while the enlarged anchor set stays at or under
  `ANCHOR_SHARE_OF_CAP` (one half) of the cap -- so escalation can grow the coverage
  guarantee's depth but can never crowd the ordinary ranking below half of a capped
  report (World3 pre-bound: `k` escalated to `MAX_ANCHOR_K` and 140 of 200 slots were
  anchors; see "Measured"). Remaining slots are filled by the existing competitive-first
  mean-relative order. Presentation order is unchanged (competitive-first, mean-relative,
  magnitude tie-break, content key), only membership changes; ids stay content-derived.

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


After Phase 3 (release, Apple M-series under Asahi,
`examples/ltm_fallback_eval`, both generators over the same simulated
results). Recall is measured against the `Auto` run's REPORTED loop list --
the retention survivors ranked competitive-first by mean relative score and
capped at `MAX_LOOPS` -- so "recall@K" is the share of the exact top-K that
appears in the fallback's reported list. Step-dominant coverage sweeps every
saved step `t in 1..step_count` with an active exact loop, takes the exact
loop with the largest `|rel_scores[t]|`, and asks whether the fallback
reported it.

World3-03 (401 saved steps, 15 stocks, exact run 0.41 s, 200 reported loops;
399 of 400 steps carry an active loop, with 42 distinct step-dominant loops):

| weight | time (s) | loops | recall@1 | recall@10 | recall@50 | recall@100 | recall@200 | step-dominant covered |
|---|---|---|---|---|---|---|---|---|
| `ClampedLogAbs` | 0.020 | 59 | 0.00 (0/1) | 0.10 (1/10) | 0.06 (3/50) | 0.03 (3/100) | 0.04 (7/200) | 0.10 (41/399) |
| `RelativeLinkScore` | 0.020 | 48 | 0.00 (0/1) | 0.10 (1/10) | 0.06 (3/50) | 0.03 (3/100) | 0.03 (6/200) | 0.10 (41/399) |
| `HopCount` | 0.011 | 23 | 0.00 (0/1) | 0.00 (0/10) | 0.00 (0/50) | 0.00 (0/100) | 0.01 (3/200) | 0.02 (7/399) |

C-LEARN v77 (251 saved steps, 116 stocks, exact run 0.037 s, 153 reported
loops; every step carries an active loop, with 2 distinct step-dominant
loops):

| weight | time (s) | loops | recall@1 | recall@10 | recall@50 | recall@100 | recall@153 | step-dominant covered |
|---|---|---|---|---|---|---|---|---|
| `ClampedLogAbs` | 0.069 | 97 | 1.00 (1/1) | 0.70 (7/10) | 0.76 (38/50) | 0.67 (67/100) | 0.63 (97/153) | 1.00 (250/250) |
| `RelativeLinkScore` | 0.069 | 97 | 1.00 (1/1) | 0.70 (7/10) | 0.76 (38/50) | 0.67 (67/100) | 0.63 (97/153) | 1.00 (250/250) |
| `HopCount` | 0.066 | 72 | 1.00 (1/1) | 0.70 (7/10) | 0.64 (32/50) | 0.47 (47/100) | 0.47 (72/153) | 1.00 (250/250) |

`ClampedLogAbs` is the measured best and stays `FallbackWeight::DEFAULT`: it
weakly dominates `RelativeLinkScore` on every World3 column (7 vs 6 of the
exact top-200, 59 vs 48 loops proposed) and ties it exactly on C-LEARN, where
the two report the identical 97 loops. `HopCount` -- the score-blind control
-- is worse than both everywhere, which is the result that says the score
weighting is doing work at all.

Two things these numbers do NOT establish, and both make them a lower bound
on generator recall rather than a measurement of it:

- **Every recall@K is bounded above by `min(loops, K)/K`.** World3's fallback
  reports 59 loops against a 200-loop reference, so recall@200 could not have
  exceeded 0.30 whatever it found. A low recall here is partly a statement
  about the reported list's LENGTH, which the fallback's own retention filter
  and cap decide, and not only about which cycles the search proposed.
- **Both sides' retention denominators are their own candidate sets.** The
  enumeration path normalizes against the universe; the fallback normalizes
  against whatever it discovered. Two loops with identical raw scores can
  therefore be retained on one path and dropped on the other. That is also
  why no "share of universe partition mass the fallback holds" statistic is
  reported: the two paths' partition totals are different denominators, so
  the ratio would compare incomparable quantities.

Recall against the full retention-survivor set (2,979 on World3, which the
public API's cap hides) is the notebook audit's job rather than this
harness's.

The gap between the two models is the design's own thesis restated as a
measurement: C-LEARN's runtime graph holds 162 ever-simultaneously-active
cycles, so a per-(stock, step) shortest-path sample recovers most of what
matters and never misses a dominant loop; World3's holds 150,827, and the
same sample recovers almost none of the exact ranking. The fallback is a
usable degradation for a sparse runtime graph and an explicit sample for a
dense one, which is what `enumeration_complete == false` is there to say.

After Phase 3b (release, Apple M-series under Asahi,
`examples/ltm_fallback_eval` over the same simulated results). The fallback is
configured on four axes -- weight, seed policy, which cycles a completed pair
of searches closes, and the tie-break -- and the tables sweep them. Labels are
`weight | seeds | closures`, where `log`/`rel`/`hop`/`shift` are
`ClampedLogAbs`/`RelativeLinkScore`/`HopCount`/`ShiftedLogAbs`,
`stock`/`+stockless`/`all-scc` are `Stocks`/`StocksAndStocklessSccs`/`AllSccNodes`,
and `in-edge`/`every-edge` are `SeedInEdges`/`EveryEdge`; a trailing
`| node-id tie` switches `FallbackTieBreak` to `NodeId` (default `Hops`).
`candidates` is the sweep's own proposed-cycle count before retention or the
cap (`DiscoveryResult::fallback_candidates`), printed next to `loops` (the
REPORTED count after retention and the cap) so the two are never conflated: a
strategy's candidate volume and what survives to the report are different
numbers, and a low recall can be a statement about either.

Step-dominant coverage is counted per (COMPETING GROUP, saved step) pair, not
per step -- a correction to how this harness itself measured the statistic
(review finding, 2026-08-18): a GLOBAL argmax over the exact reported loops
measures nothing, since a loop alone in its partition -- or the sole retention
SURVIVOR of one, once the reported list is capped -- has `|rel_scores[t]| == 1`
at every active step by construction, so it wins any global argmax it is
active for regardless of raw magnitude; on both models here a global argmax
named a non-competing loop at literally every step, which is why the earlier
"step-dominant covered" numbers below were an artifact of the harness rather
than a measurement. A group is "competing" iff at least two REPORTED exact
loops share its partition; within a competing group, at each saved step where
some member is active, the pair `(group, step)` is covered iff the fallback
reported that group's step-max loop.

World3-03 (401 saved steps, 15 stocks, exact run 0.409 s, 200 reported loops;
399 competing (group, step) pairs, with 50 distinct dominant loops across
them):

| weight \| seeds \| closures | time (s) | candidates | loops | recall@1 | recall@10 | recall@50 | recall@100 | recall@200 | step-dominant pairs covered |
|---|---|---|---|---|---|---|---|---|---|
| log \| stock \| in-edge | 0.022 | 90 | 59 | 0.00 (0/1) | 0.10 (1/10) | 0.06 (3/50) | 0.03 (3/100) | 0.04 (8/200) | 0.09 (34/399) |
| rel \| stock \| in-edge | 0.021 | 68 | 48 | 0.00 (0/1) | 0.10 (1/10) | 0.06 (3/50) | 0.03 (3/100) | 0.04 (7/200) | 0.09 (34/399) |
| hop \| stock \| in-edge | 0.012 | 25 | 23 | 0.00 (0/1) | 0.00 (0/10) | 0.00 (0/50) | 0.00 (0/100) | 0.01 (3/200) | 0.02 (6/399) |
| shift \| stock \| in-edge | 0.021 | 34 | 34 | 0.00 (0/1) | 0.00 (0/10) | 0.00 (0/50) | 0.00 (0/100) | 0.01 (3/200) | 0.02 (6/399) |
| log \| stock \| every-edge | 0.141 | 2150 | 200 | 0.00 (0/1) | 0.30 (3/10) | 0.18 (9/50) | 0.10 (10/100) | 0.15 (31/200) | 0.13 (50/399) |
| rel \| stock \| every-edge | 0.133 | 1925 | 200 | 0.00 (0/1) | 0.10 (1/10) | 0.12 (6/50) | 0.07 (7/100) | 0.13 (26/200) | 0.12 (48/399) |
| hop \| stock \| every-edge | 0.084 | 478 | 147 | 0.00 (0/1) | 0.10 (1/10) | 0.06 (3/50) | 0.03 (3/100) | 0.05 (10/200) | 0.10 (39/399) |
| shift \| stock \| every-edge | 0.092 | 801 | 200 | 0.00 (0/1) | 0.10 (1/10) | 0.10 (5/50) | 0.05 (5/100) | 0.07 (15/200) | 0.10 (41/399) |
| log \| +stockless \| in-edge | 0.022 | 90 | 59 | 0.00 (0/1) | 0.10 (1/10) | 0.06 (3/50) | 0.03 (3/100) | 0.04 (8/200) | 0.09 (34/399) |
| log \| +stockless \| every-edge | 0.141 | 2150 | 200 | 0.00 (0/1) | 0.30 (3/10) | 0.18 (9/50) | 0.10 (10/100) | 0.15 (31/200) | 0.13 (50/399) |
| shift \| +stockless \| every-edge | 0.096 | 801 | 200 | 0.00 (0/1) | 0.10 (1/10) | 0.10 (5/50) | 0.05 (5/100) | 0.07 (15/200) | 0.10 (41/399) |
| log \| +stockless \| every-edge \| node-id tie | 0.141 | 2160 | 200 | 0.00 (0/1) | 0.30 (3/10) | 0.18 (9/50) | 0.10 (10/100) | 0.15 (31/200) | 0.13 (50/399) |
| shift \| +stockless \| every-edge \| node-id tie | 0.093 | 801 | 200 | 0.00 (0/1) | 0.10 (1/10) | 0.10 (5/50) | 0.05 (5/100) | 0.07 (15/200) | 0.10 (41/399) |
| log \| all-scc \| in-edge | 0.159 | 331 | 170 | 0.00 (0/1) | 0.10 (1/10) | 0.10 (5/50) | 0.06 (6/100) | 0.07 (15/200) | 0.11 (45/399) |
| log \| all-scc \| every-edge | 1.027 | 4066 | 200 | 0.00 (0/1) | 0.30 (3/10) | 0.26 (13/50) | 0.15 (15/100) | 0.20 (39/200) | 0.13 (51/399) |

C-LEARN v77 (251 saved steps, 116 stocks, exact run 0.037 s, 153 reported
loops; 750 competing (group, step) pairs across several partitions, with 44
distinct dominant loops across them -- the recall@153 column is over the 153
loops that exist):

| weight \| seeds \| closures | time (s) | candidates | loops | recall@1 | recall@10 | recall@50 | recall@100 | recall@153 | step-dominant pairs covered |
|---|---|---|---|---|---|---|---|---|---|
| log \| stock \| in-edge | 0.063 | 100 | 97 | 1.00 (1/1) | 0.70 (7/10) | 0.76 (38/50) | 0.67 (67/100) | 0.63 (97/153) | 0.84 (628/750) |
| rel \| stock \| in-edge | 0.062 | 100 | 97 | 1.00 (1/1) | 0.70 (7/10) | 0.76 (38/50) | 0.67 (67/100) | 0.63 (97/153) | 0.84 (628/750) |
| hop \| stock \| in-edge | 0.059 | 75 | 72 | 1.00 (1/1) | 0.70 (7/10) | 0.64 (32/50) | 0.47 (47/100) | 0.47 (72/153) | 0.84 (627/750) |
| shift \| stock \| in-edge | 0.062 | 75 | 72 | 1.00 (1/1) | 0.70 (7/10) | 0.64 (32/50) | 0.47 (47/100) | 0.47 (72/153) | 0.84 (628/750) |
| log \| stock \| every-edge | 0.146 | 159 | 150 | 1.00 (1/1) | 1.00 (10/10) | 1.00 (50/50) | 1.00 (100/100) | 0.98 (150/153) | 1.00 (750/750) |
| rel \| stock \| every-edge | 0.146 | 155 | 146 | 1.00 (1/1) | 1.00 (10/10) | 1.00 (50/50) | 0.96 (96/100) | 0.95 (146/153) | 1.00 (750/750) |
| hop \| stock \| every-edge | 0.130 | 147 | 139 | 1.00 (1/1) | 1.00 (10/10) | 0.94 (47/50) | 0.90 (90/100) | 0.90 (138/153) | 0.98 (734/750) |
| shift \| stock \| every-edge | 0.139 | 147 | 139 | 1.00 (1/1) | 1.00 (10/10) | 0.94 (47/50) | 0.90 (90/100) | 0.90 (138/153) | 0.98 (734/750) |
| log \| +stockless \| in-edge | 0.064 | 100 | 97 | 1.00 (1/1) | 0.70 (7/10) | 0.76 (38/50) | 0.67 (67/100) | 0.63 (97/153) | 0.84 (628/750) |
| log \| +stockless \| every-edge | 0.148 | 159 | 150 | 1.00 (1/1) | 1.00 (10/10) | 1.00 (50/50) | 1.00 (100/100) | 0.98 (150/153) | 1.00 (750/750) |
| shift \| +stockless \| every-edge | 0.139 | 147 | 139 | 1.00 (1/1) | 1.00 (10/10) | 0.94 (47/50) | 0.90 (90/100) | 0.90 (138/153) | 0.98 (734/750) |
| log \| +stockless \| every-edge \| node-id tie | 0.147 | 159 | 150 | 1.00 (1/1) | 1.00 (10/10) | 1.00 (50/50) | 1.00 (100/100) | 0.98 (150/153) | 1.00 (750/750) |
| shift \| +stockless \| every-edge \| node-id tie | 0.140 | 147 | 139 | 1.00 (1/1) | 1.00 (10/10) | 0.94 (47/50) | 0.90 (90/100) | 0.90 (138/153) | 0.98 (734/750) |
| log \| all-scc \| in-edge | 0.105 | 141 | 138 | 1.00 (1/1) | 0.70 (7/10) | 0.94 (47/50) | 0.94 (94/100) | 0.90 (138/153) | 0.86 (644/750) |
| log \| all-scc \| every-edge | 0.484 | 162 | 153 | 1.00 (1/1) | 1.00 (10/10) | 1.00 (50/50) | 1.00 (100/100) | 1.00 (153/153) | 1.00 (750/750) |

The chosen default is
`FallbackConfig { weight: ClampedLogAbs, seeds: StocksAndStocklessSccs,
closures: EveryEdge, tie_break: Hops }`. Under it `examples/ltm_discovery_bench`
reports a 0.142 s fallback on World3 against a 0.406 s exact run and 0.147 s on
C-LEARN against 0.037 s -- inside the constraint that the fallback cost at
most half the exact World3 run and at most 0.2 s on either model. AC3.1 is
unaffected: the `Auto` path still enumerates in 0.406 s and 0.037 s, complete
on both.

Reading the axes:

- **Weight.** `ClampedLogAbs` weakly dominates `RelativeLinkScore` on every
  column at BOTH closure settings (World3 every-edge: 31 vs 26 of the exact
  top-200, 50 vs 48 pairs covered; C-LEARN every-edge: 150 vs 146 loops, 1.00
  vs 0.96 at recall@100), so the weight conclusion does not depend on the
  closure choice. `HopCount`, the score-blind control, stays worst or
  tied-worst everywhere, which is the result that says the score weighting is
  doing work at all.
- **Closures.** Closing on every edge is still the lever, and a bigger one
  than the earlier (three-axis) measurement showed: it is not a bigger sample
  of the same kind -- the in-edge family emits the minimum-weight cycle
  through the SEED, and one shortest-path tree holds one route per node, so
  parallel routes to the same node are unreachable however many seeds or
  steps are swept; the every-edge family emits, for each edge both trees
  reach, the minimum-weight cycle through the seed AND that edge -- the
  strength-weighted analogue of edge coverage. World3's recall of the exact
  top-200 goes 8 -> 31 and its step-dominant coverage 34 -> 50 of 399
  (candidate volume goes 90 -> 2,150); C-LEARN's goes 97 -> 150 of 153, with
  recall@1, @10, @50 and @100 all exactly 1.00 and step-dominant coverage
  628 -> 750 of 750 (perfect).
- **Seeds.** `StocksAndStocklessSccs` is measurement-neutral on both models --
  every column matches `Stocks`, which is evidence that neither carries a
  non-trivial SCC holding no stock -- and it closes AC1.3's gap by
  construction rather than by luck: a cycle whose state hides in a module
  level or a `PREVIOUS` lag between auxes has no stock to seed from, and one
  representative per stockless component reaches it. What is NOT measured here
  is its cost on a model that does carry such components; that cost is bounded
  at one extra search pair per component per saved step. `AllSccNodes` did not
  earn its place: with every-edge closures it costs 1.027 s on World3 -- 2.5x
  the exact enumeration it stands in for, and 4,066 candidates against 2,150 --
  for recall@200 of 39 against 31, and with in-edge closures it costs 0.159 s
  for 15 against 8, worse per unit time than stock-seeded every-edge's 31 at
  0.141 s. It stays selectable and unused.
- **The hop tie-break is the default and is measurement-neutral.** Both arms
  (`FallbackTieBreak::{Hops, NodeId}`) were measured at the chosen
  seeds/closures with both contending weights: every recall column and every
  step-dominant count is identical between them on both models (World3 31/200
  and 50/399 under `ClampedLogAbs` either way; C-LEARN 150/153 and 750/750
  either way). What `Hops` buys is inside the zero-weight plateau
  `ClampedLogAbs` creates -- with a third of a real graph's active links
  weighing exactly 0, many cycles tie, and the tie-break decides among them on
  cycle length rather than on node numbering -- which is a statement about
  the model rather than about interning order, at no measured cost.
- **`ShiftedLogAbs` was measured and rejected.** `w = ln(step max finite |s|)
  - ln|s|` keeps the relative gain among super-unit links that the clamp
  discards (an edge of gain 1000 costs less than one of gain 2), which is the
  distinction World3's long high-gain dominant loops turn on -- so it was the
  obvious hypothesis for the clamp's low recall. Measured at the chosen
  seeds/closures it is WORSE: World3 recall@200 15 against 31 and
  step-dominant 41 against 50 of 399; C-LEARN 138/153 against 150/153 and
  734/750 against 750/750, where its rows are IDENTICAL to `HopCount`'s on
  C-LEARN (72/139/138/734 either way). The mechanism is visible in the sum:
  `Sigma w = L * ln(max) - ln(product)`, and on these models `ln(max)` per hop
  (max |s| ~1e4-1e6 on World3, up to 1e14 on C-LEARN) dwarfs any product
  term, so the shifted arm degenerates toward a hop count (identically on
  C-LEARN; close to it on World3's every-edge closures, where it still beats
  `HopCount` -- 15 vs 10 at recall@200 -- but trails `ClampedLogAbs` by the
  same margin `HopCount` does). It stays selectable as a documented negative
  result.

**k-best via edge penalty was considered and not adopted.** World3's chosen
strategy already reports the full 200-loop cap while spending 0.141 s of a
~0.2 s budget, so a second penalized round does not fit inside the stated
constraint. It is also not the obvious next lever: because the reported list
is AT the cap, recall@200 is no longer bounded above by that list's length
(the caveat the Phase 3 tables carried), so 31 of 200 is a genuine overlap
measurement -- roughly 117x what a uniform 200-cycle sample of World3's
150,827-cycle universe would be expected to hit (`200 * 200 / 150827 ~ 0.27`
expected by chance).

---

### Audit numbers (`notebooks/build_ltm_discovery_audit.py`)

An independent pure-Python re-implementation of the enumerator, the scoring,
the retention filter and the ranking -- written from this document rather than
translated from the Rust -- reproduces the engine exactly on both models:

| | World3-03 | C-LEARN v77 |
|---|---|---|
| union-graph edges (self-edges dropped) | 258 of 428 | 2,965 of 21,042 |
| non-trivial union SCCs | one, 135 nodes | three of 65, twelve of 2 |
| elementary cycles ever simultaneously active | 150,827 | 162 |
| enumeration time (pure Python) | 9.3 s | < 0.1 s |
| retention survivors (>= 0.1% peak share) | 2,979 | 153 |
| engine reported loops | 200 (cap binds) | 153 (cap does not bind) |
| engine loops absent from the independent universe | 0 | 0 |
| reported-list overlap with the independent ranking | 200/200 | 153/153 |
| max relative difference, raw loop scores | 0.000e+00 | 0.000e+00 |
| max absolute difference, relative loop scores | 0.000e+00 | 0.000e+00 |
| step-dominant coverage, competing groups | 399/399 (100%) | 750/750 (100%) |

The universe count, the survivor count and the reported set all match the
engine bit for bit, and both score series are bit-identical -- so AC3.2's
"survivors and their scores are bit-identical" holds against an external
oracle and not only against a golden.

Two findings about the coverage statistic, one of which the cap answers:

- **The step-dominant coverage the cap has to hold is 100%, and holds it.**
  Before the coverage-aware cap World3 missed 17 of 399 steps: at each, the
  loop with the largest `|relative score|` within its competing partition was
  enumerated, was retained, and was then dropped by `MAX_LOOPS`. Those 17 are
  the measurement AC5.1's anchoring drives to 0 (see "After Phase 4" below),
  and the audit re-measures it on every regeneration.
- **A GLOBAL argmax over relative score measures nothing.** A loop alone in
  its normalization group is its own denominator, so its relative score is
  `+/-1` at every active step by construction. On World3 the global argmax is
  a non-competing loop at 399 of 399 steps (giving a global coverage of
  0/399), and on C-LEARN at 250 of 250. Any coverage statistic -- including
  the one Phase 4's cap is judged by -- has to be taken WITHIN a competing
  group, which is what AC5.1 already says and what the audit now demonstrates
  the necessity of.

After Phase 4 (release, Apple M-series under Asahi,
`examples/ltm_discovery_bench` and the regenerated audits):

| Model | Discovery time | Reported | Step-dominant coverage (competing groups) | Reported loops changed |
|---|---|---|---|---|
| C-LEARN v77 | 0.037 s | 153 | 750/750 (100%) | 0 of 153 |
| World3-03 | 0.407 s | 200 | 399/399 (100%) | 49 of 200 |

AC3.1 still holds (World3 0.407 s < 1.0 s, C-LEARN 0.037 s < 0.2 s, both
complete). The rank phase on World3 is 32.0 ms of the 407, of which the
coverage-aware selection is 3.5 ms -- one scan over 2,979 survivors x 401
steps. C-LEARN's whole rank phase is 0.44 ms and its selection returns
immediately: 153 loops is under the cap, so nothing is selected against.

What the cap does on World3, in slots: 50 survivors are some step's dominant
loop in their competing partition (`k = 1`), 96 are within a step's top two,
and 140 within a step's top three -- all three fit under `MAX_LOOPS`, so the
escalation runs to `MAX_ANCHOR_K` and 140 of the 200 slots are anchors, with
the remaining 60 filled by the mean-relative ranking. That is what moves 49 of
the reported 200: 151 loops are reported by both selections, 20 of them at a
different rank, and the presentation order is the same competitive-first
mean-relative ranking either way. C-LEARN's cap does not bind, so its report
is unchanged loop for loop.

Universe-based competing classification (AC5.2) reclassifies little on these
two models -- 2,967 of World3's 2,979 survivors and 141 of C-LEARN's 153
compete -- because both models' partitions are large. Its effect is on the
models the enumeration prunes hardest, where a partition can hold one survivor
and many sub-threshold siblings whose mass is still in its denominator.

The audit's independent re-implementation reproduces the engine's selection
exactly on both models (200/200 and 153/153 reported-list overlap), so the
anchoring, the escalation bound and the tie rule are checked against a second
implementation written from this document rather than only against the Rust.

After the anchor-share review fix (review finding, 2026-08-18; release, Apple
M-series under Asahi, `examples/ltm_discovery_bench` and the regenerated
audits): 140 of 200 World3 slots anchored left only 60 (30%) for the ordinary
mean-relative ranking -- a coverage guarantee crowding the ranking claim down
far past what AC5.1 asks for (its guarantee is `k = 1`; everything past it is
a bonus, not a requirement). `select_reported` now bounds escalation past
`k = 1` to `ANCHOR_SHARE_OF_CAP` (one half) of the cap: on World3, `k = 2`'s
anchor set (96) fits within `200 * 0.5 = 100` but `k = 3`'s (140) does not, so
escalation stops at `k = 2`, anchoring 96 of the 200 slots and leaving 104 to
the mean-relative ranking. That moves 25 of the reported 200 relative to a
plain (non-anchored) mean-relative truncation -- down from 49 under the
unbounded rule this replaces, confirmed by re-measuring the OLD rule
side-by-side against the same ranked list (which reproduces this section's
original 49-loop figure exactly, the cross-check that the two measurements are
comparable) -- so 175 loops are reported under both the share-bounded and the
plain rankings, and
the presentation order is still the same competitive-first mean-relative
ranking under every variant. C-LEARN's cap does not bind, so `k`-escalation
never runs there and its report is unaffected by the bound (unchanged loop for
loop, as above). The audit's independent re-implementation (updated with the
same `ANCHOR_SHARE_OF_CAP` rule) still reproduces the engine's selection
exactly on both models (200/200 and 153/153 reported-list overlap), and the
100% step-dominant coverage figure above is unchanged: the `k = 1` guarantee
the cap has to hold is exempt from the share bound by construction.

(Phase 6 still to fill in.)
