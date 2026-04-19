# LTM circuit-cap lift: design options

Date: 2026-04-18
Branch: `ltm-perf-enable-always`
Owner: architect (team `ltm-perf-unleash`)

## Background

`MAX_LTM_CIRCUITS = 100_000` (`src/simlin-engine/src/ltm.rs:32`) protects the
downstream LTM pipeline, not the cycle enumerator. After the work on
`reduce-ltm-mem`, Johnson's DFS on WRLD3 finishes in ~1.2 s under 500 MiB.
The cliff is materialization of synthetic variables in
`db_ltm.rs:model_ltm_variables` (`src/simlin-engine/src/db_ltm.rs:2050`) and
everything downstream of it: parsing per-equation text, compilation into
bytecode, and per-timestep VM execution.

For WRLD3 (1,863,803 elementary circuits, one 166-node SCC, 15 stocks,
mean circuit length 47, ~483 out-edges):

- Exhaustive mode emits **~2 synthetic vars per circuit** (`loop_score`
  and `rel_loop_score`, plus link scores for edges that touch any loop).
  At 1.86M loops that is ~3.7M variables.
- **Relative loop score is O(P²) in equation text** per partition. Each of
  the P loops in a partition produces an equation
  `SAFEDIV(loop_score_i, SUM(ABS(loop_score_1..P)), 0)` that names every
  other loop in the partition (`ltm_augment.rs:424`). For WRLD3's single
  ~1.86M-loop partition, that is roughly 3.5 trillion character references
  of equation text — the parser does not even reach compilation.
- Loop score text is linear in loop count but each equation is a product
  of ~47 link-score references. Total text ~100 MB for WRLD3. Parsable
  but expensive.
- Discovery mode (`ltm_discovery_mode = true`) already avoids all per-loop
  synthesis: it emits link scores for every edge (~483 for WRLD3) and
  reconstructs loops post-simulation via the strongest-path search in
  `ltm_finding.rs`, capped at `MAX_LOOPS = 200`.

WASM target is 4 GiB linear memory. Any design that leaves WRLD3 needing
tens of GiB of equation text or bytecode is non-viable.

## Goals

Let LTM stay on for models at WRLD3 scale and above, without abandoning
LTM semantics (per-edge per-timestep link scores, per-cycle loop scores,
per-partition relative loop scores, per-loop polarity, LOOPSCORE /
PATHSCORE builtin support).

## Options

### A. Auto-switch to discovery mode above a circuit threshold

**Description.** When the element-level cycle enumerator reports more
circuits than a configurable threshold (say 5 000 — well below the current
100 k cap, comfortably above Bass/Yeast/Inventory-scale test models),
`model_ltm_variables` emits discovery-mode output instead of exhaustive
output: link scores for all edges, no loop_score / rel_loop_score
variables. Loops are ranked and reported post-simulation via
`discover_loops_with_graph` with its existing `MAX_LOOPS = 200` cap and
`MIN_CONTRIBUTION = 0.1 %` filter. Papers (Schoenberg 2020 & 2020.1)
describe this as the production two-tier strategy; Simlin already has
both modes but no auto-switch — documented as divergence #2 in
`docs/design/ltm--loops-that-matter.md`.

**Spirit-of-LTM fidelity.**
- Preserved: link scores per edge per timestep; loop scores and relative
  loop scores for the top-200 discovered loops (computed post-sim);
  polarity; discovery-mode pathway / composite scores for sub-models.
- Approximated: loops outside the top 200 are not returned. The paper
  documents this as intended — 200 is "generous enough that users get
  what matters".
- Dropped: per-loop score as a **live runtime variable**. LOOPSCORE /
  PATHSCORE builtins need a resolution path (see Risks).

**Expected win.** WRLD3: ~3.7M synthetic vars → ~500 (link scores for
every edge); equation text goes from multi-TB to sub-MB. Models
with < 5 k loops keep the exhaustive path and see no behavior change.

**Implementation complexity.** Small-medium.
- `db_ltm.rs:2111` — gate the `!is_discovery` branch on
  `circuits_result.len() <= AUTO_DISCOVERY_THRESHOLD`. Already have
  circuit count without paying the full enumeration because the
  enumerator bails on budget exhaustion and returns `TruncatedByBudget`.
- Emit a user-visible diagnostic when auto-flip happens so the caller
  sees "ran in discovery mode due to loop count".
- LOOPSCORE / PATHSCORE resolution: these builtins take a loop id /
  variable list at author time. For discovery mode, synthesize a
  per-builtin-request loop_score variable on demand — the specific loop
  is cheap to materialize.

**Risks.**
- LOOPSCORE / PATHSCORE semantics in discovery mode must be vetted end-
  to-end. If an author references a loop that is not among the discovered
  top-200, the post-sim value is undefined in today's code. The builtin
  path should either force exhaustive-for-that-loop materialization or
  return a principled "not-discovered" signal.
- Behavior change is user-visible: same model can yield different loop
  sets when it grows past threshold. Needs a single clear diagnostic and
  a deterministic threshold.
- CI: existing LTM integration tests for small models keep the exhaustive
  path; add at least one discovery-triggered test (synthetic or WRLD3
  subset) to lock the flip.

**Interactions.** Composes with B and D. Substitutes for C and for any
top-K materialization scheme. Independent of G.

---

### B. Compute `rel_loop_score` post-simulation, not as a live variable

**Description.** Drop the quadratic text-blowup equation in
`ltm_augment.rs:424` from the compile-time pipeline. Keep `loop_score` as
a live variable (its text is O(N × mean_loop_len), not O(N²)). Compute
`rel_loop_score[loop_i, t] = loop_score[loop_i, t] /
Σ_j∈partition |loop_score[j, t]|` in Rust after simulation, using the
saved results buffer. The analysis API (`analysis.rs`,
`ltm_finding.rs:rank_and_filter`) already does partition-scoped
aggregation; extend it to emit relative scores as a second pass.

**Spirit-of-LTM fidelity.**
- Preserved: every LTM quantity is still available to downstream
  analysis; rel_loop_score exists, just not as a runtime variable in the
  VM's offsets table.
- Approximated: nothing.
- Dropped: ability to reference `$⁚ltm⁚rel_loop_score⁚…` from **inside a
  user equation** at simulation time. Current callers that only read it
  from `Results` after the run are unaffected.

**Expected win.** Kills the O(P²) text-explosion term outright. For a
WRLD3-shape model with a single P = 1.86M partition, compile-time text
drops from ~3.5 TB to ~100 MB (just the loop_score product equations).
Runtime VM still stores P × save_steps = 1.86M × 1 000 ≈ 15 GB of loop
score data, which does not fit in WASM. Therefore B **alone is not
sufficient for WRLD3** — it is a necessary piece inside option A's
exhaustive branch, where P stays below threshold.

**Implementation complexity.** Medium.
- Delete rel_loop_score generation from `generate_loop_score_variables`
  (`ltm_augment.rs:131`).
- Add a post-sim pass that, given the saved loop_score timeseries for a
  partition, computes the per-loop relative score timeseries. Two known
  downstream consumers read rel_loop_score by name via VM `get_series`:
  - `src/libsimlin/src/analysis.rs:408`
    (`simlin_analyze_get_rel_loop_score` FFI)
  - `src/simlin-engine/src/layout/mod.rs:3865` (feedback-loop importance
    in layout metadata)
  Both read full timeseries post-sim; neither uses the value
  mid-simulation. Both can switch to the computed accessor.
- Tests: `db_ltm_unified_tests.rs` and `db_ltm_module_tests.rs` assert
  presence of rel_loop_score synthetic vars. Update them or assert the
  post-sim equivalent instead.

**Risks.**
- LOOPSCORE builtin returning absolute loop score today — does any
  equivalent read relative score? None in tree. If one is added later,
  it must materialize on demand like LOOPSCORE.
- Moves LTM boundary between "VM-live" and "post-sim". Cleaner, but the
  analysis.rs pipeline has to always run before a caller asks for
  relative scores. Today it does.

**Interactions.** Composes with A (discovery already skips rel_loop_score
vars; this extends the same logic to exhaustive mode). Composes with D.
Independent of others.

---

### D. Per-partition (per-SCC) circuit budget instead of global cap

**Description.** `find_loops_with_limit` currently decrements one global
budget counter across every SCC (`ltm.rs:1190`). Replace with a per-SCC
budget: each SCC gets either a fixed cap (say 10 k) or a quota computed
from size. A model with 20 small SCCs no longer has one SCC starve
everyone else when it blows past budget.

**Spirit-of-LTM fidelity.**
- Preserved: identical semantics. All loops within budget are emitted.
- Approximated / dropped: nothing extra beyond what the global cap
  already drops.

**Expected win.** Helps models with many small partitions. **Does not
help WRLD3** — its loops all live in a single 166-node SCC. Primarily a
robustness / fairness improvement for the shape of models where one
pathological partition shouldn't kill loop detection for the rest.

**Implementation complexity.** Small.
- `ltm.rs:1209` — per-SCC budget decrement.
- Choose the per-SCC policy. A simple starting point: global budget /
  nontrivial_scc_count, floored at some minimum (e.g. 1 000).

**Risks.** Low. CI: add a test with two SCCs where one has > budget and
the other has a handful of loops, verify the small SCC's loops survive.

**Interactions.** Composes cleanly with A (small SCCs take exhaustive,
oversize SCC alone flips to discovery, model gets a hybrid report) and
with B.

---

### C. Top-K in-sim loop materialization with structural ranking

**Description.** Enumerate all circuits structurally (cheap post
`reduce-ltm-mem`), then pick a structural top-K (smallest first;
containing most stocks; shortest path between named stocks; or similar)
and emit `loop_score` / `rel_loop_score` synthetic vars only for those K.
Loops outside K are metadata-only; users read their scores post-sim via
the same strongest-path / link-score reconstruction used by discovery
mode.

**Spirit-of-LTM fidelity.**
- Preserved: every link score live; top-K loops live.
- Approximated: loops outside K reconstructed post-sim.
- Dropped: loops outside K have no in-sim `loop_score` variable, so user
  equations / LOOPSCORE calls that name those loops would have to
  fall back to on-demand materialization.

**Expected win.** For WRLD3 at K = 1 000: ~3 k synthetic vars, same order
of magnitude as option A's win.

**Implementation complexity.** Medium. Ranking heuristic is the
interesting question — and is exactly what discovery mode's strongest-
path search answers dynamically, not structurally.

**Risks.** Heuristic quality. A structural ranking is blind to dynamics;
loops that are small but low-gain will crowd out larger, high-gain loops.
Discovery mode already handles this correctly via runtime scores. Unless
there is a need to pick loops **before** running a simulation (there is
not — LTM vars are generated after the model compiles but they do not
affect the non-LTM run), C collapses to "a worse version of A".

**Interactions.** Subsumed by A. Only interesting if some reason
requires in-sim loop-score variables for specific loops — which LOOPSCORE
/ PATHSCORE already handles via explicit author request.

---

### E. Cycle basis / fundamental cycles (research direction, likely dead end)

**Description.** For WRLD3's 166-node SCC with E edges in-SCC, a cycle
basis has E − 166 + 1 fundamental cycles. Any elementary cycle is a
symmetric-difference (edge-set XOR) combination of basis cycles.

**Spirit-of-LTM fidelity.** LTM loop score is **multiplicative** over a
cycle's signed link scores. Symmetric difference preserves edge sets but
does **not** preserve a multiplicative score: if cycle A has edges
{e1, e2, e3} and B has {e3, e4, e5}, then A ⊕ B has edges {e1, e2, e4, e5}
and score
`|ls(e1)|·|ls(e2)|·|ls(e4)|·|ls(e5)|` — not any algebraic combination of
score(A) and score(B). So basis cycles **cannot** reconstruct per-cycle
LTM scores.

**Expected win.** Enumeration-side only: 171 basis cycles instead of
1.86M. Zero win for the downstream pipeline that is the actual
bottleneck.

**Recommendation.** Do not pursue. Note honestly: this is a research
direction if someone wants to redefine the LTM score in a basis-friendly
way (e.g. an additive-log score), but that is a new method, not an
implementation optimization of the existing one.

---

### F. Per-partition aggregate loop score (one score per partition)

**Description.** Replace per-circuit `loop_score` with a single
per-partition score (e.g. max |link_score_in_partition|, or
Σ |link_score|).

**Spirit-of-LTM fidelity.** Destroys the per-cycle contract. LTM is
"loops that matter" — losing the per-loop granularity means users cannot
see which loop dominates at time t. Dropped.

---

### G. Trie / shared-prefix compression of circuit storage

**Description.** Store circuits as a trie to compress shared prefixes.

**Spirit-of-LTM fidelity.** Preserved.

**Why out of scope here.** Addresses enumeration-side memory, which
`reduce-ltm-mem` already brought under 500 MiB. Does not touch the
downstream synthetic-variable cliff that the 100 k cap protects against.
Orthogonal improvement; pursue separately if needed after the core lift.

---

## Ranked recommendations

1. **A + B together.** A is the load-bearing change: auto-flip to
   discovery mode for large-circuit models, which is exactly the paper's
   prescribed two-tier strategy. B cleans up the worst downstream scaling
   term (O(P²) relative-loop-score text) and lets the exhaustive branch
   of A stay cheap even near the threshold. Together they let LTM stay on
   for WRLD3-scale models.
2. **D as a follow-up.** Per-SCC budget is cheap, low-risk, and helps a
   real-world shape (multi-SCC models) that the global cap handles
   poorly. Not a substitute for A.
3. **Revisit the auto-flip threshold after instrumentation.** Task #1's
   diagnostician is finding where the downstream pipeline actually
   cliffs as the cap rises. The threshold for A should be set from those
   numbers, not picked abstractly.

## Do not do

- **E (cycle basis).** Semantically incompatible with multiplicative LTM
  scoring. Symmetric difference of cycles is not a cycle in the LTM
  sense.
- **F (per-partition aggregate score).** Breaks the per-loop contract.
  "Loops that matter" means per loop, not per partition.
- **G (trie storage of circuits).** Addresses the wrong stage — the cap
  exists to protect downstream synthesis, not enumeration memory.
- **C (top-K structural ranking).** Dynamics, not structure, decide
  which loops matter. Discovery mode's post-sim strongest-path already
  does the dynamic ranking correctly; a structural ranking would be
  strictly worse without a concrete reason to materialize specific
  loops as in-sim variables (LOOPSCORE / PATHSCORE already covers the
  explicit case).

## Open questions for the team lead

- Threshold for A: awaiting task #1's diagnostic numbers.
- LOOPSCORE / PATHSCORE behavior in discovery mode: on-demand per-loop
  materialization vs. a principled "not-in-top-200" signal. Prefer the
  former; confirm.
- Whether `rel_loop_score` is referenced from inside any user equation
  (grep says no — only post-sim reads in libsimlin's analysis FFI and in
  layout metadata). If a future builtin wants it live, materialize on
  demand like LOOPSCORE.

## Resolution

Resolved 2026-04-18 on branch `ltm-perf-enable-always`:

- **Option A (auto-flip):** shipped.  `MAX_LTM_SCC_NODES = 50` in
  `src/simlin-engine/src/ltm.rs` gates `model_ltm_variables`; SCCs above
  the threshold flip to discovery mode with a user-visible diagnostic.
- **Option B (post-sim rel_loop_score):** shipped.  Computation lives
  in `src/simlin-engine/src/ltm_post.rs`; the compile-time O(P²)
  equation-text cliff is gone.
- **Option D (per-SCC budget):** not needed.  Auto-flip already covers
  multi-SCC starvation for every model in the test corpus.
- **Cap:** `MAX_LTM_CIRCUITS` removed.  Enumeration now runs uncapped
  against `usize::MAX`; the `TruncatedByBudget` signal remains on the
  `_with_limit` APIs for future stress tests.

See `2026-04-18-ltm-cap-lift-diagnosis.md` and
`2026-04-18-ltm-cap-lift-validation.md` for the resolved versions with
measurements.
