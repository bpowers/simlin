# LTM discovery: union-graph circuit enumeration as the primary candidate generator

## Problem

Discovery-mode LTM is a three-stage pipeline: candidate generation (find cycles
worth scoring), exact scoring (per-step product of recorded link scores), and
filter/rank (`MIN_CONTRIBUTION` retention, competitive-first ranking,
`MAX_LOOPS` cap). Only candidate generation is lossy, and a ground-truth audit
(2026-08-10; the audits are regenerable via
`notebooks/build_ltm_discovery_audit.py --model clearn|wrld3`) showed it has two regimes:

- C-LEARN: the per-step strongest-first DFS is effectively exhaustive (all 162
  ever-active cycles found; the 9 unreported ones are correctly dropped by
  retention). Exact.
- World3: the per-node expansion cap (`max(1, 4096/|SCC|)` ~ 36 on its ~120-node
  active SCCs) saturates on 100% of (stock, step) searches, silently. The DFS
  surfaces ~5k of 330,059 real cycles through the same deterministic
  strongest-first funnel at every step. The final 200 overlap the true top-200
  (by the engine's own ranking statistic) by 47/200; the true most-dominant
  loop at a step is absent from the report at 227/399 steps; a missed loop
  peaks at 27% partition dominance.

## Design

Discovery runs after the simulation, when the set of edges that ever carried
signal is observable — the information advantage compile-time exhaustive mode
cannot have. Exploit it:

1. **Union graph**: restrict the parsed link-score edge set to edges whose
   |score| is nonzero (finite) at >= 1 saved step in `1..step_count` (step 0 is
   all-NaN, matching the DFS's skip).
2. **Activity-bitset-pruned elementary-circuit enumeration**, once, on the
   union graph (not per step: per-step graphs are ~99% identical, so per-step
   enumeration is ~T x the work for zero additional recall). Each edge carries
   a bitset over saved steps; the DFS maintains the running AND along its path
   and prunes when it empties — so only *ever-simultaneously-active* cycles
   are emitted, which kills the disjoint-phase union blowup at the root. This
   is a Tiernan-style min-root search (each cycle emitted exactly once, from
   its minimum node id) rather than Johnson, because path-dependent pruning
   breaks Johnson's unblocking invariant. Self-edges emit singleton circuits
   (the current DFS finds them for stock nodes; C-LEARN has 49 ever-active
   self-loops — `SAMPLE IF TRUE` latches).
   Correctness: a loop with nonzero score at step t has every link nonzero at
   t (score is a product), so every scorable loop is an ever-simultaneously-
   active elementary cycle of the union graph. The converse cycles score 0
   everywhere and are dropped by retention ("self-filtering").
3. **Streaming two-pass retention** (never materializes per-loop state for the
   full universe): pass 1 accumulates per-partition per-step totals
   Sum(|score_j[t]|) over ALL enumerated circuits; pass 2 recomputes each
   circuit's series and retains those whose peak `|s|/total` >= MIN_CONTRIBUTION
   (plus every circuit traversing a module node, conservatively, since module
   link overrides can change a loop's final score away from the raw product).
   NaN link values follow the engine's rules (loop score NaN at that step,
   excluded from totals and from the loop's own retention test).
4. **Materialize only survivors** (plus cross-agg stitched loops, produced by
   the existing `collect_agg_petals`/`stitch_cross_agg_petals` core over the
   full enumerated set, id-based; their mass is added to the totals) through
   the *unchanged* FoundLoop construction — exact scoring, module-input
   override series, synthetic trim, reported-cycle dedup.
5. **Ranking uses the full-universe denominators**: `rank_and_filter` accepts
   optional external per-partition totals. Relative scores are then normalized
   against the whole universe's mass (matching exhaustive-mode semantics where
   the enumerated set IS the universe). "Competing vs. solo" is computed over
   the loops given to `rank_and_filter` — i.e. retention survivors — so
   never-active phantom co-members cannot flip a genuinely-solo loop to
   "competing with mean rel ~ 1.0" (the demotion-neutering hazard).
6. **Bounded and observable**: enumeration carries a circuit budget, a visit
   budget (structural blowups where paths wander without closing), and the
   caller's wall-clock deadline. If any trips, discovery falls back to the
   existing per-step DFS unchanged. `DiscoveryResult` grows two flags:
   `enumeration_complete` (candidates are provably the full ever-active
   universe) and `expansion_cap_saturated` (the DFS fallback hit its per-node
   cap somewhere — today's silent saturation, made visible).

## Honest boundaries

- Completeness is at *saved-step* resolution with respect to the *recorded*
  series: sub-save-step activity is invisible (shared with the DFS), and for a
  loop entering a multi-output module the activity test reads the module
  composite, whose max-abs fold lets a NaN pathway shadow a finite one
  (shared with the DFS; a composite that is 0 at every step does imply every
  per-port override is 0, so the finite case is safe).
- Stockless cycles (state hidden in module levels or PREVIOUS lags) become
  discoverable where the stock-seeded DFS structurally could not find them;
  they resolve to `NormGroup::Solo` (GH #750) and rank after competing loops.
  This is a deliberate widening toward the exhaustive universe.
- A model with no parent stocks is still analyzed rather than declared an
  empty universe: the enumerator needs no stock seeds and the fallback's
  default seed policy (`StocksAndStocklessSccs`) seeds stockless SCCs
  directly, so a stockless stateful cycle resolves to the `NormGroup::Solo`
  group above instead of being skipped (2026-08-18 correction; the original
  plan's `stocks.is_empty()` early return declared such a model's universe
  empty, which contradicted the very widening this plan describes).
- Reported relative scores change on models where discovery was previously
  lossy (denominators now include the full universe's mass). That is the
  point, but it is a semantic change.

## Measured feasibility

Pure-Python prototypes of exactly this pipeline: C-LEARN enumeration 1.3s /
162 cycles; World3 enumeration ~30s + scoring ~14s for 330k cycles x 401
steps. The Rust implementation is expected to be 1-2 orders faster; the
`ltm_discovery_bench` example measures it on repo models.
