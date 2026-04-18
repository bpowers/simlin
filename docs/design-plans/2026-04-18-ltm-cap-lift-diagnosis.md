# LTM downstream cap-lift diagnosis

Date: 2026-04-18
Branch: `ltm-perf-enable-always`

## Summary (TL;DR)

Raising `MAX_LTM_CIRCUITS` above ~1.86M (WRLD3's true circuit count) exposes
**two distinct downstream cliffs** inside `model_ltm_variables`
(`src/simlin-engine/src/db_ltm.rs:2051`). Both must be avoided for LTM to
be always-on at WRLD3 scale and above.

- **Cliff A (undocumented until now): `build_element_level_loops`
  materialization.** Between the end of `loop_circuits` (RSS 411 MiB) and
  the entry of `generate_loop_score_variables` (RSS 17,533 MiB), the
  process allocates ~17.1 GB converting 1,863,803 indexed circuits into
  `Vec<Loop>` with full `Link` vectors (mean 47 links each). This alone
  blows WASM's 4 GiB linear-memory ceiling; **any** downstream LTM work
  is impossible while this path runs.
- **Cliff B (architect's O(P²) prediction confirmed).** `rel_loop_score`
  emits one equation per loop that names every other loop in the
  partition inside a `SAFEDIV(..., SUM(ABS(...)))` denominator. Measured
  at WRLD3's single 1,863,779-loop partition: **75,303,840 bytes per
  equation (~75.3 MB), ~40 bytes per reference**. Full emission would
  need ~140 TB of equation text. The process was SIGKILLed by the OS at
  ~57.6 GiB RSS after emitting only 512 of 1,863,779 rel_loop_score
  equations (<0.03% complete).

Loop score emission is linear and comparatively cheap: **~4.15 KB per
loop, ~7.7 GB total text.** Not a binding constraint by itself, but
Cliff A runs before it.

Recommendation: implement architect's **Option A first** (auto-flip to
discovery mode above a threshold) so neither cliff is reached on dense
models. The gate must bind on **largest-partition size**, not total
circuit count, and must decide **before** calling
`build_element_level_loops`.

## Measurement setup

- Branch: `ltm-perf-enable-always`.
- Harness: `src/simlin-engine/examples/ltm_full_bench.rs` (commit
  `5eaf5a1c`). Stages: parsed, synced, ltm_enabled, causal_edges,
  element_edges, loop_circuits, ltm_variables, compile. Memory read from
  `/proc/self/status` at each stage boundary.
- Runtime cap override: `simlin_engine::ltm::set_max_ltm_circuits` (commit
  `5eaf5a1c`). Lets the bench vary the cap without rebuilding the
  documented `MAX_LTM_CIRCUITS` constant.
- Trace instrumentation inside `generate_loop_score_variables` behind
  `LTM_BENCH_TRACE=1` (commit `567a61bc`, denser early cadence in
  `2ea9c2b5`). Emits cumulative loop_score / rel_loop_score equation
  bytes and RSS at power-of-two sample points plus every 10,000 loops.
- Model: `test/metasd/WRLD3-03/wrld3-03.mdl` (1 model, 311 root
  variables, 483 causal edges, 15 stocks, 1 non-trivial 166-node SCC).
- Run command:
  `LTM_BENCH_ABORT_MIB=6000 LTM_BENCH_TRACE=1 cargo run --release -p
  simlin-engine --example ltm_full_bench -- test/metasd/WRLD3-03/wrld3-03.mdl
  2000000`.  The 6 GiB abort ceiling is a stage-boundary check only
  (`Tracker::record`), so it does not fire inside the long-running
  `ltm_variables` stage; the OS OOM killer is what terminates the
  process.
- Raw output: `/tmp/ltm-diag/cap-02000000-trace.log` (~19 KB,
  uncommitted; kept in tmp for reproducibility).

## Cap sweep 100K - 1M: the zero-circuits artifact

The prior cap sweep (`/tmp/ltm-diag/cap-{100000,250000,500000,1000000}.log`)
shows `circuits=0` and `vars=0` at every cap below WRLD3's true circuit
count. This is **not** a scaling signal; it is a consequence of the
enumerator contract:

- `CausalGraph::find_indexed_circuits_with_limit` returns
  `Err(TruncatedByBudget)` when the DFS would exceed the budget.
- `find_indexed_circuits` (default wrapper) converts that to the empty
  pair `(Vec::new(), Vec::new())`.
- `model_element_loop_circuits` returns the empty pair verbatim to
  `model_ltm_variables`.
- `model_ltm_variables` sees `circuits_result.is_empty()` at
  `db_ltm.rs:2113` and returns early with no synthetic variables.

The existing cap=100K does not cap WRLD3's loop list at 100K; it causes
the enumerator to bail and report zero loops, at which point the
downstream pipeline does no work. The cap is binary, not gradual.

Consequence for the cap lift: any partial cap is unusable without either
(a) a spill-to-discovery-mode fallback (architect's option A) or (b) a
redesigned enumerator that returns a partial list when the budget is
exhausted. The existing enumerator would need non-trivial changes to
support (b), and its interaction with Johnson's unblock logic means a
"first N elementary circuits" list is not well-defined in the
mathematical sense.

## Cap 2M: the two cliffs

### Stage-level memory (enumeration cap = 2,000,000 circuits)

| Stage                    | wall (ms) | VmPeak (MiB) | VmRSS (MiB) | note                                  |
|--------------------------|----------:|-------------:|------------:|---------------------------------------|
| parsed                   |       2.7 |         8.41 |        6.59 | 311 root vars                          |
| synced                   |       0.3 |         9.25 |        6.95 | root='main'                            |
| ltm_enabled              |       0.0 |         9.25 |        6.95 | flag=true                              |
| causal_edges             |       2.8 |        10.76 |        8.41 | src_nodes=302, total_edges=483         |
| element_edges            |       0.2 |        11.18 |        8.68 | src_nodes=302, total_edges=483         |
| loop_circuits            |    1511.3 |       467.08 |      411.49 | circuits=1,863,803, unique_names=166   |
| generate_loop_score start |         — |            — |   17,533.00 | +17.1 GB RSS vs end of loop_circuits   |

The 17.1 GB gap between `loop_circuits` exit and
`generate_loop_score_variables` entry is **Cliff A**. That stretch is
`model_ltm_variables` lines 2066-2354: `model_causal_edges` (small),
source-variable fetch (small), `model_element_loop_circuits`
(memoized from the stage above), `build_element_level_loops` at
`db_ltm.rs:2123` (builds 1.86M `Loop` structs with mean 47 `Link`
entries each, plus polarity analysis and module stock enrichment per
loop), and the Part-1 link-score-generation loop at `db_ltm.rs:2354`
(iterates all loop-bound edges). The dominant allocator in that stretch
is `build_element_level_loops`: each `Link` carries two cloned
`Ident<Canonical>` strings and a `LinkPolarity`, so a 1.86M x 47-link
fanout yields ~175M allocated `Ident<Canonical>` clones on top of the
per-loop `Vec<Link>` and `Vec<Ident<Canonical>>` stock list.

### loop_score pass (linear)

| Loops emitted | cum loop_score bytes | bytes/loop | RSS (MiB) |
|--------------:|---------------------:|-----------:|----------:|
|             1 |                6,603 |          — |  17,533.0 |
|        10,000 |           48,321,750 |       4832 |  17,533.0 |
|       100,000 |          537,521,716 |       5375 |  17,533.0 |
|       500,000 |        2,417,938,086 |       4836 |  17,533.0 |
|     1,000,000 |        4,473,046,093 |       4473 |  18,923.4 |
|     1,500,000 |        6,498,958,720 |       4333 |  20,843.4 |
|     1,860,000 |        7,717,401,936 |       4149 |  22,893.1 |

Linear: ~4.15 KB per loop_score equation on average. Total 7.7 GB of
text across 1.86M loops. RSS is flat for the first ~770K loops because
the buffers fit inside the 17.5 GiB already reserved by Cliff A; RSS
grows ~1-2 MiB/1000 loops once the reservation is exceeded. loop_score
is **not** the binding constraint, but it compounds on top of Cliff A.

### rel_loop_score pass (quadratic per partition)

Partitions at this stage: 2 (sizes 1,863,779 and 24). The large
partition is the 166-node SCC. Every rel_loop_score equation in that
partition is a `SAFEDIV(loop_score_i, SUM(ABS(loop_score_1..P)), 0)`
that names all P loops in the denominator.

| Rel equations emitted | cum rel bytes | bytes/eqn (rolling) | RSS (MiB) |
|----------------------:|--------------:|--------------------:|----------:|
|                     1 |    75,303,840 |          75,303,840 |  23,156.8 |
|                     2 |   150,607,680 |          75,303,840 |  23,228.6 |
|                     4 |   301,215,360 |          75,303,840 |  23,372.4 |
|                     8 |   602,430,720 |          75,303,840 |  23,659.6 |
|                    16 | 1,204,861,447 |          75,303,840 |  24,234.2 |
|                    32 | 2,409,722,903 |          75,303,840 |  25,383.2 |
|                    64 | 4,819,445,815 |          75,303,840 |  27,681.4 |
|                   128 | 9,638,891,668 |          75,303,840 |  32,277.6 |
|                   256 | 19,277,783,444 |         75,303,840 |  41,470.1 |
|                   512 | 38,555,566,996 |         75,303,840 |  57,550.2 |

**Each equation is exactly 75,303,840 bytes** (independent of `i`
because the denominator always names all P loops). Per-loop cost is
linear in partition size; total cost for the partition is quadratic:
`bytes_total ≈ ~40 × P² ≈ 140 TB at P=1,863,779`.

The OS SIGKILLed the process at RSS ~57.6 GiB after emitting only 512
equations (~0.03% complete). The bench's in-process abort ceiling does
not fire mid-stage.

## What cliffs first

**For WRLD3 specifically, Cliff A (17 GB in `build_element_level_loops`)
cliffs first** because it runs before any rel_loop_score emission.

**For the general case, Cliff B (O(P²) rel_loop_score text) binds at
much smaller partition sizes.** Threshold math: per-partition
rel_loop_score text ≈ `40 × P²` bytes. A few thresholds:

| Partition size P | rel_loop_score text per partition |
|-----------------:|----------------------------------:|
|            1,000 |                             40 MB |
|            5,000 |                              1 GB |
|           10,000 |                              4 GB |
|           50,000 |                            100 GB |
|        1,863,779 |                            140 TB |

For a 4 GiB WASM budget, the rel_loop_score pass alone caps partition
size at roughly P ≤ ~10,000 even if Cliff A were fixed. For Cliff A, the
binding constraint is total loop count across all partitions (roughly
~250K loops on a 4 GiB budget at the measured per-loop allocation of
~9 KB).

## Micro vs structural

Both cliffs are **structural** for any model where the largest partition
has more than a few thousand loops. Micro-optimizations alone will not
close the gap:

- Interning `Ident<Canonical>` inside `Link` would halve `Link` size (~2
  String clones per Link) but not change the O(circuits × mean_length)
  scaling of `build_element_level_loops`. Order-of-magnitude win at
  best, ~3-5x at realistic intern hit rates. Still 5-7 GB on WRLD3.
- Streaming the rel_loop_score text (never materializing all equations
  at once) does not fix the per-equation size. Each single
  rel_loop_score equation is 75 MB at WRLD3 scale; parsing any one of
  them requires at least that much working memory.
- The per-equation size itself is P × per-reference-bytes, which is
  determined by the SAFEDIV-over-SUM structure mandated by the spec.
  Compressing the reference text (e.g., short integer IDs) only shaves
  a constant factor.

The structural fixes are the architect's options A and B. Option A
(auto-flip to discovery above a partition-size threshold) short-circuits
both cliffs; option B (post-sim rel_loop_score) shaves the quadratic
term on the exhaustive branch for models that stay below the threshold.

## Recommendations (feeding task #3)

1. **Highest-leverage bet: option A, auto-flip to discovery mode above a
   largest-partition-size threshold.** Inside `model_ltm_variables`,
   check the SCC structure of the element-level graph *before* calling
   `build_element_level_loops` at `db_ltm.rs:2123`. If
   `max_scc_size > THRESHOLD`, take the `is_discovery` branch instead.
   The SCC computation is cheap (`ltm_mem_bench` measured 166-node SCC
   detection in ~100 ms on WRLD3; the existing
   `CyclePartitions::compute_cycle_partitions` at `ltm.rs` is the
   canonical caller). Proposed initial threshold: largest SCC > 50
   nodes, *or* element-level circuit count > 10,000, whichever fires
   first. Re-measure on WRLD3 and a scale-down synthetic to confirm the
   flip triggers when expected.
2. **Option B, post-sim rel_loop_score, remains worthwhile on the
   exhaustive branch.** Even below the auto-flip threshold, any
   partition with P > ~5,000 loops will see rel_loop_score compile-time
   text > 1 GB. Computing rel_loop_score post-sim from the already-
   saved loop_score timeseries is bounded by `P × save_steps × 8 bytes`
   instead of `40 × P²` bytes.
3. **Option D (per-SCC budget) is orthogonal and cheap.** Keep as
   follow-up; it helps multi-SCC models where one pathological SCC
   should not starve the rest. Does not help WRLD3 (single non-trivial
   SCC).

### Open questions for the implementation

- **Cliff A bookkeeping**: the 17 GB in `build_element_level_loops`
  includes the Part-1 link-score generation loop at `db_ltm.rs:2354`.
  Before deciding option A is sufficient, instrument the sub-stages
  of that function to confirm `build_element_level_loops` itself is
  the dominant allocator (hypothesized) vs the link-score loop.
- **UX for the flip**: users running a mid-size model should see a
  clear, stable diagnostic when auto-flip fires. The existing
  `ltm_discovery_mode` flag on `SourceProject` is a plausible carrier;
  confirm with the diagram/UI side before wiring through.
- **Test coverage**: add at least one integration test that trips
  auto-flip on a synthetic dense graph and one that stays on the
  exhaustive branch. The existing `wrld3_ltm_compilation_finishes_in_time`
  test should continue to pass and should exercise the auto-flip.

## Raw data

- `/tmp/ltm-diag/cap-{100000,250000,500000,1000000}.log` — zero-circuits-
  below-cap sweep, artifact of binary truncation.
- `/tmp/ltm-diag/cap-02000000.log` — initial 2M run, truncated mid-
  `ltm_variables` by OS SIGKILL (no trace).
- `/tmp/ltm-diag/cap-02000000-trace.log` — 2M run with
  `LTM_BENCH_TRACE=1`; the numbers in this document are drawn from this
  file.
