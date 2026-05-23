# Layout Quality Evaluation and Hill-Climbing Harness Design

## Summary

This work builds a closed-loop measurement and tooling harness around `simlin-engine`'s
automatic diagram layout, so that an agent (or human) can improve layout quality with
evidence instead of guesswork. The core is two **pure** Rust modules that hold all the
logic: a *quality-metric core* (`layout/metrics.rs`) that scores a laid-out diagram on
explicit, scale-free aesthetic cost terms -- node overlap, connectors running through
nodes, label overlap, edge crossings, sprawl, edge-length unevenness, and aspect ratio --
and collapses them to a single `weighted_cost` scalar; and a *statistics core*
(`layout/eval_stats.rs`) that treats a layout's quality as a distribution over random
seeds, summarizing it with medians, percentiles, a corpus-wide geomean, and a Mann-Whitney
U significance test (the way Go's `benchstat` compares benchmark runs). Crucially, the
metric is computed on the *same geometry the PNG renderer draws*, so a layout's score can
never disagree with how it actually looks. An imperative shell -- an on-demand example
binary (`examples/layout_eval.rs`) -- composes these cores: it sweeps a curated corpus of
models, lays each out across many seeds, scores them, renders the best/median/worst (and any
hand-authored reference view) to PNG, and writes a metrics table plus an HTML contact-sheet.

The architecture exists to enable a tight iteration loop: change a layout parameter or code
path, run the sweep, read the geomean delta *and look at the rendered contact-sheet*, then
keep or revert based on whether the change is statistically significant and visually better.
The scalar `weighted_cost` is the hill to climb; the rendered images are the guardrail
against optimizing the number while degrading the picture (Goodhart's law); and a small set
of human-vs-AI reference pairs is the objective check that the metric agrees with human
taste. With that loop in place, the design takes only the first, smallest algorithm step --
"Rung 0," re-pointing seed selection to rank by the full metric instead of crossings alone
-- and protects the gain with a fast deterministic CI guard. Rungs 1-3 (parameter search,
a metric-driven search objective, and new layout passes) are documented as the forward path
the harness is built to support, not built here.

## Definition of Done

This work builds the measurement and tooling infrastructure that lets an agent
iteratively improve `simlin-engine`'s automatic diagram layout. It defines *what a good
layout is* (an explicit, geometry-accurate quality metric) and *how to judge outputs* (a
corpus sweep that renders and statistically scores layouts), then takes the first
improvement step (Rung 0). The layout algorithm itself is not redesigned beyond Rung 0;
rungs 1-3 are documented as the forward path.

Today the layout engine judges a layout by exactly one quantity -- edge-crossing count
(`annealing.rs` simulated-annealing cost; `select_best_layout` seed ranking) -- and there
is no in-repo way to *see* a generated layout outside the browser. This design closes both
gaps.

1. **A pure `LayoutMetrics` module** (`src/simlin-engine/src/layout/metrics.rs`) computes
   scale-free aesthetic *cost* terms (0 = ideal) from a `StockFlow` view, on the same
   geometry the PNG renderer draws: `node_overlap`, `node_connector_overlap`,
   `label_overlap`, `crossings`, `sprawl`, `edge_length_cv`, and `aspect_penalty`, plus
   reserved zero-weighted structure terms. `weighted_cost(&MetricWeights) -> f64` collapses
   them to one scalar to minimize.

2. **Edge crossings are counted on real geometry** -- Arc links sampled to polylines
   instead of straight chords -- fixing the chord approximation `count_view_crossings`
   (`mod.rs`) applies to `Link`/Arc shapes today (flow polylines are already
   segment-sampled). MultiPoint links currently render to nothing; see Additional
   Considerations.

3. **A Rust in-tree corpus sweep driver** (`src/simlin-engine/examples/layout_eval.rs`)
   runs over a curated `test/` corpus: for each model it generates layouts across multiple
   independent seeds, computes `LayoutMetrics` for each, renders the best/median/worst
   layouts to PNG, and -- where the model ships a hand-authored view -- also scores and
   renders that view as a reference. No pysimlin or other-binding surface is added.

4. **The sweep reports statistically**: per-model median + spread over the seed samples,
   a corpus geomean-of-medians aggregate, the production best-of-k cost, and a
   baseline-vs-candidate comparison using a Mann-Whitney U significance test -- emitted as a
   metrics table (JSON) and an HTML contact-sheet (best/median/worst per model with score
   breakdowns), written to a gitignored output directory under `target/`.

5. **Metric weights are calibrated and committed**: initial weights set from the
   failure-mode priorities (overlap + crossings dominant; sprawl/aspect moderate;
   structure ~0), refined against rendered examples, and validated by a reference-pair
   check -- on agreed human-vs-AI model pairs the metric scores the human layout lower
   (better) than the worse machine layout.

6. **Rung 0 is wired in**: `select_best_layout` (`mod.rs`) ranks the candidate seeds by
   `weighted_cost` (using the accurate crossing count) instead of crossings-only.

7. **A deterministic CI regression guard**: a fast test over a few tiny models asserts
   `weighted_cost` stays at or below a committed threshold, and the reference-pair ordering
   is encoded as a test -- both within the workspace's 3-minute test-time budget.

8. **The hill-climbing ladder (rungs 1-3) is documented** as the forward path (parameter
   search; metric-driven annealing cost; new layout passes), naming the seam each rung
   touches. (Satisfied by this plan's Additional Considerations -- no implementation task.)

### Out of scope
- Redesigning the layout algorithm beyond Rung 0 (rungs 1-3 are documented, not built).
- Exposing metrics or rendering through pysimlin or any non-Rust binding.
- A preference-judging UI or a trained preference model (the explicit metric is the chosen
  signal; human preference enters only as up-front calibration).
- SD-structure metrics as *weighted* terms (chain straightness, loop readability) -- the
  fields exist but are zero-weighted initially, since these were de-prioritized.

## Acceptance Criteria

### layout-quality-eval.AC1: Metric terms are geometry-correct and scale-free
- **layout-quality-eval.AC1.1 Success:** Two node boxes overlapping by a known area yield a `node_overlap` equal to the known overlap fraction.
- **layout-quality-eval.AC1.2 Success:** Pairwise-disjoint nodes yield `node_overlap` = 0.
- **layout-quality-eval.AC1.3 Success:** A connector whose polyline passes through a non-incident node box contributes to `node_connector_overlap`; one that avoids all non-incident boxes yields 0.
- **layout-quality-eval.AC1.4 Success:** Two label boxes overlapping by a known area yield a matching `label_overlap`; non-overlapping labels yield 0.
- **layout-quality-eval.AC1.5 Success:** `aspect_penalty` is 0 inside the target aspect band and positive outside it (a 1x10 bbox is penalized; a ~4:3 bbox is not).
- **layout-quality-eval.AC1.6 Success:** `weighted_cost` equals the exact linear combination Σ wᵢ·termᵢ for given weights.
- **layout-quality-eval.AC1.7 Edge:** An empty or single-element view yields all-zero terms with no NaN or divide-by-zero.
- **layout-quality-eval.AC1.8 Success:** Uniformly scaling all coordinates leaves every normalized term unchanged within tolerance (scale invariance).

### layout-quality-eval.AC2: Crossings are counted on real geometry
- **layout-quality-eval.AC2.1 Success:** Two connectors that cross once yield a crossing count of 1; connectors sharing an endpoint yield 0.
- **layout-quality-eval.AC2.2 Success:** An Arc connector that visually crosses another edge is counted via polyline sampling, on a constructed case where the straight-chord approximation does not count it. (MultiPoint links currently render to nothing, so faithfully counting them is deferred with that renderer gap -- see Additional Considerations.)
- **layout-quality-eval.AC2.3 Success:** The crossing count is invariant under translation and rotation of the whole view.

### layout-quality-eval.AC3: Corpus sweep produces renders and scores
- **layout-quality-eval.AC3.1 Success:** `cargo run --release --example layout_eval` runs over the curated corpus and exits 0.
- **layout-quality-eval.AC3.2 Success:** It writes `metrics.json` with per-model term breakdowns + `weighted_cost` and corpus aggregates.
- **layout-quality-eval.AC3.3 Success:** It writes `index.html` referencing best/median/worst PNGs per model with score breakdowns.
- **layout-quality-eval.AC3.4 Success:** Models shipping a hand-authored view get a reference render + score alongside the auto-layout.
- **layout-quality-eval.AC3.5 Success:** All artifacts are written under `target/` (gitignored); nothing is committed.
- **layout-quality-eval.AC3.6 Edge:** A model that fails to lay out or render is reported and skipped, not fatal to the sweep.

### layout-quality-eval.AC4: Statistical reporting and comparison
- **layout-quality-eval.AC4.1 Success:** Per model, M seeds produce M samples; the report includes median + spread (p25/p75) and the best-of-k production proxy.
- **layout-quality-eval.AC4.2 Success:** The corpus aggregate is the geomean of per-model medians.
- **layout-quality-eval.AC4.3 Success:** A baseline-vs-candidate run reports per-model and aggregate deltas, each with a Mann-Whitney U p-value / significance verdict.
- **layout-quality-eval.AC4.4 Success:** `geomean`, median/percentile, and Mann-Whitney U match known reference values.
- **layout-quality-eval.AC4.5 Edge:** Identical baseline and candidate yield a zero aggregate delta and a non-significant verdict.

### layout-quality-eval.AC5: Calibration is validated objectively
- **layout-quality-eval.AC5.1 Success:** Committed default `MetricWeights` give overlap and crossings the dominant weights and the reserved structure terms zero weight.
- **layout-quality-eval.AC5.2 Success:** On the agreed human-vs-AI reference pairs, `weighted_cost(human) < weighted_cost(ai)` under the committed weights (encoded as a test).

### layout-quality-eval.AC6: Rung 0 selection uses the full metric
- **layout-quality-eval.AC6.1 Success:** `select_best_layout` picks the lowest-`weighted_cost` candidate, verified on constructed candidates where the lowest-cost layout has *more* crossings than another candidate (so the choice differs from crossings-only).
- **layout-quality-eval.AC6.2 Success:** The existing layout test suite (`tests/layout.rs`, `layout_tests.rs`, `layout_review_tests.rs`) passes unchanged with the new selection.

### layout-quality-eval.AC7: CI regression guard
- **layout-quality-eval.AC7.1 Success:** A deterministic test over a few tiny models asserts `weighted_cost` <= a committed threshold and completes well within the test-time budget.
- **layout-quality-eval.AC7.2 Failure:** Raising a guard model's `weighted_cost` above the threshold makes the test fail.

### layout-quality-eval.AC8: Cross-cutting
- **layout-quality-eval.AC8.1 Success:** A fixed seed reproduces a byte-identical layout (determinism), distinct from the M-seed statistical sampling.
- **layout-quality-eval.AC8.2 Success:** Additional Considerations documents rungs 1-3 and names the seam each touches. (Satisfied by this design document itself; no implementation phase.)

## Glossary

- **System dynamics (SD) / stock-and-flow model**: A modeling approach that represents a
  system as stocks (accumulations) connected by flows (rates of change) and feedback links.
  Simlin builds, simulates, and visualizes these models; their visual form is the "diagram"
  whose layout this work scores.
- **StockFlow / `StockFlow` view**: The engine's data structure for a model diagram -- the
  collection of `ViewElement`s (and their positions) that make up one visual view of a
  model. The metric takes a `&StockFlow` as input.
- **`ViewElement`**: A single positioned item in a `StockFlow` view (a stock, flow, auxiliary
  variable, connector, alias, etc.). Layout assigns each one a position.
- **Connector / Arc / MultiPoint / `Flow.points`**: Connectors are the links drawn between
  elements. They are not always straight: an Arc is a curved link, a MultiPoint connector
  bends through intermediate points, and a flow's pipe follows `Flow.points`. The crossing
  count and metric sample these into polylines so curved/bent geometry is measured
  faithfully.
- **SFDP**: The force-directed graph layout algorithm used to place nodes (`layout/sfdp.rs`),
  treating links as springs and nodes as mutually repelling charges. Its tunable parameters
  (`k`, `c`, `p`, spacing constants) are the target of the documented Rung 1 parameter
  search.
- **Force-directed layout**: The broader family of layout algorithms (SFDP is one) that
  positions nodes by simulating attractive/repulsive forces until the system settles.
- **Simulated annealing (SA)**: The optimization pass (`layout/annealing.rs`) that refines a
  layout by randomly perturbing it and accepting changes probabilistically, with the
  acceptance probability cooling over time. It currently minimizes edge crossings only;
  Rung 2 would feed it the full `weighted_cost`.
- **Edge crossings**: Places where two connectors visually intersect -- a primary source of
  diagram clutter, and today the *only* quantity layout optimizes.
- **`count_view_crossings`**: The existing function (`mod.rs`) that counts crossings. Today it
  approximates connectors as straight chords; this work refactors it to count on sampled
  polylines so arcs and bends are handled correctly.
- **`LAYOUT_SEEDS` / seed sampling**: Production runs layout from four fixed random seeds
  (`[42, 123, 456, 789]`) and keeps the best result. Because layout is deterministic per
  seed but its quality varies across seeds, the sweep instead samples *many* seeds to
  characterize the quality distribution rather than a single lucky/unlucky result.
- **`select_best_layout`**: The function (`mod.rs`) that picks the winning candidate among
  the seed runs. Rung 0 re-points it from "fewest crossings" to "lowest `weighted_cost`."
- **`LayoutMetrics` / `weighted_cost` / `MetricWeights`**: The new quality-metric types.
  `LayoutMetrics` holds one cost term per aesthetic concern (0 = ideal, all scale-free);
  `MetricWeights` is one weight per term; `weighted_cost` is their weighted sum `Σ wᵢ·termᵢ`
  -- the single scalar an optimizer minimizes.
- **`render_png` / resvg**: `render_png` (`diagram/render_png.rs`, behind the `png_render`
  feature) rasterizes a diagram to a PNG; resvg is the Rust SVG-rendering library it uses.
  Because the engine's SVG output is byte-identical to the product's TypeScript renderer,
  the PNG faithfully reflects the real UI.
- **geomean (geometric mean)**: The aggregate used to combine per-model median costs across
  the corpus. Unlike the arithmetic mean, it averages ratios fairly so one large-cost model
  cannot dominate the corpus score.
- **Mann-Whitney U test**: A non-parametric significance test that decides whether two
  samples differ. It is used to judge whether a baseline-vs-candidate cost difference is real
  signal or seed noise, without assuming the cost distributions are normal.
- **benchstat**: A Go tool that compares benchmark runs by reporting center, spread, and a
  significance test over many samples. The statistics core deliberately mirrors its approach
  for layout quality.
- **best-of-k**: A "production proxy" statistic -- the minimum cost over k seeds -- that
  mirrors what production actually ships (best of the fixed seed set), reported alongside the
  full distribution.
- **Reference pair (human-vs-AI)**: An agreed pairing of a hand-authored ("human") layout and
  a machine-generated ("AI") layout of the same model. The metric is validated by requiring
  `weighted_cost(human) < weighted_cost(ai)` -- an objective check that it agrees with human
  taste.
- **Contact-sheet**: The generated `index.html` report -- a grid showing each model's
  best/median/worst renders (and any reference view) with their score breakdowns, sorted
  worst-first -- inspected every iteration as the visual guardrail.
- **"Rungs" / hill-climbing ladder**: The staged forward path for improving layout. Rung 0
  (built here) changes only seed selection; Rungs 1-3 (documented, not built) are parameter
  search, a metric-driven search objective, and new layout passes -- each "rung" a discrete,
  measurable step up the quality hill.
- **Goodhart('s law)**: "When a measure becomes a target, it ceases to be a good measure" --
  i.e., any single fitness scalar will eventually be gamed. The contact-sheet renders,
  visible per-term breakdowns, and reference-pair test are the design's guards against it.
- **Functional core / imperative shell (FCIS)**: An architectural pattern that isolates pure,
  side-effect-free logic (here, `metrics.rs` and `eval_stats.rs`) from the I/O-performing
  shell (here, the `layout_eval.rs` example). The cores are heavily unit/property tested; the
  shell stays thin.
- **salsa**: The incremental computation framework backing the engine's model database; the
  sweep driver syncs the salsa DB before laying out a model, reusing the path that the
  existing `tests/layout.rs` uses to load corpus models.

## Architecture

The system has three parts, split along the functional-core / imperative-shell line: a
**pure metric core** and a **pure statistics core** that the **imperative sweep driver**
composes. Rendering already exists (`diagram::render_png`) and is reused unchanged.

### Quality-metric core (`layout/metrics.rs`, pure)

`compute_layout_metrics(view: &StockFlow, config: &LayoutConfig) -> LayoutMetrics` is a
pure function with no I/O. It is computed on the **same geometry the renderer draws** --
node bounding boxes, connector paths, and label boxes obtained from the `diagram` module's
existing geometry helpers (`diagram::elements`/`flow` `*_bounds`, `diagram::connector`
path, `diagram::label::label_bounds`) -- so a layout's score and its rendered PNG can never
disagree. Those helpers are `pub fn`, but their modules (`elements`, `flow`, `label`,
`connector`) are private in `diagram/mod.rs` today, so a prerequisite is exposing them
`pub(crate)` for `layout` to call. Every term is a **cost** (0 = ideal) and normalized to be scale-free, so models
of different sizes are comparable and the corpus can be aggregated.

| Term | Definition (cost; 0 = ideal) | Pain it captures |
|------|------------------------------|------------------|
| `node_overlap` | Σ pairwise node-box overlap area / Σ node area | clutter |
| `node_connector_overlap` | connector-polyline length inside non-incident node boxes / total connector length | connectors under/through nodes |
| `label_overlap` | overlap area among label boxes and label-vs-node boxes / Σ label area | clutter |
| `crossings` | connector-polyline crossings (arcs sampled) / connector count | tangled connectors |
| `sprawl` | mean connector length / characteristic node size | wasted space |
| `edge_length_cv` | stddev/mean of connector lengths | elements drifting far / unevenness |
| `aspect_penalty` | deviation of bbox aspect ratio from a target band | unviewable shape |
| `chain_straightness`, `loop_compactness` | reserved, zero-weighted | (SD structure; deferred) |

Contract:

```rust
pub struct LayoutMetrics {
    pub node_overlap: f64,
    pub node_connector_overlap: f64,
    pub label_overlap: f64,
    pub crossings: f64,
    pub sprawl: f64,
    pub edge_length_cv: f64,
    pub aspect_penalty: f64,
    pub chain_straightness: f64, // reserved, weight 0
    pub loop_compactness: f64,   // reserved, weight 0
}

pub struct MetricWeights { /* one f64 per term */ }

impl LayoutMetrics {
    /// Σ wᵢ·termᵢ — the scalar an optimizer minimizes.
    pub fn weighted_cost(&self, w: &MetricWeights) -> f64;
}
```

`node_overlap`/`node_connector_overlap`, `crossings`, and the sprawl terms pull in opposite
directions (compact vs. non-overlapping). That tension is intended: the weights set the
balance, and the overlap terms keep "minimize area" from collapsing the layout.

**Accurate crossings.** The `crossings` term, and a refactored `count_view_crossings`,
operate on connector geometry sampled to polylines (Arc links plus `Flow.points`), not
straight chords. This requires factoring the arc geometry -- currently entangled with
SVG-string emission in `connector::render_arc` (which returns a `String`) -- into a polyline
producer shared by the renderer and the metric, so both see identical geometry. This is the
highest-effort item in Phase 1, and the factor-out must keep `render_svg` byte-for-byte
unchanged (a TS-vs-Rust parity test asserts it). It both feeds the metric and fixes a latent
undercount in today's seed selection. (MultiPoint links currently render to an empty group,
so they have no drawn geometry to match; they are a known gap, not measured here.)

### Statistics core (`layout/eval_stats.rs`, pure)

Layout is deterministic at a fixed seed (RNGs are `StdRng::seed_from_u64`; no entropy
source; the `par_iter` over seeds preserves order), so a specific layout is exactly
reproducible. But a layout's *quality is a distribution over seed space*, and production
samples it at the four fixed `LAYOUT_SEEDS` and takes the min. Evaluating a change on one
fixed seed-set conflates a real improvement with seed luck. The statistics core treats
evaluation the way Go's `benchstat` treats benchmarks: many samples, center + spread, and a
significance test on differences.

```rust
pub struct MetricSample { pub seed: u64, pub metrics: LayoutMetrics, pub weighted_cost: f64 }

pub struct ModelStats {
    pub model: String,
    pub samples: Vec<MetricSample>, // one per seed
    pub median_cost: f64,
    pub spread: (f64, f64),         // e.g. (p25, p75)
    pub best_of_k_cost: f64,        // production proxy: min over k seeds
    pub best_seed: u64, pub median_seed: u64, pub worst_seed: u64,
}

pub struct CorpusReport { pub per_model: Vec<ModelStats>, pub geomean_of_medians: f64 }

/// Per-model and aggregate delta, each with a Mann-Whitney U p-value (non-parametric;
/// robust to the non-normal cost distributions layout produces).
pub fn compare(baseline: &CorpusReport, candidate: &CorpusReport) -> Comparison;
```

`geomean` (not arithmetic mean) aggregates normalized ratios across heterogeneous models so
one large-cost model can't dominate; `median`/percentiles summarize each model's
distribution; Mann-Whitney U decides whether a baseline-vs-candidate delta is signal or
noise. All are pure, table-testable functions.

### Sweep driver (`examples/layout_eval.rs`, imperative shell)

The shell loads each model in a curated corpus list (XMILE via `open_xmile` and Vensim via
`open_vensim`, as `examples/backend_bench.rs` does, then salsa-syncs the project as the
DB-backed layout tests do), and for each model:

1. Runs layout for M independent seeds, producing M `MetricSample`s (and the best-of-k
   production proxy). The per-seed seam is the existing `generate_layout_with_config`
   (`mod.rs`, `pub`) -- its single `annealing_random_seed` drives both the SFDP and
   annealing RNGs -- or the equivalent `generate` closure inside `generate_best_layout`.
2. Renders the best/median/worst layouts to PNG via `diagram::render_png` (after writing
   the generated `StockFlow` onto the model's view, which `render_png` reads as
   `views.first()`).
3. If the model file ships a non-empty hand-authored view, renders and scores that view
   untouched as a **reference**.

It then emits, to a gitignored dir under `target/layout-eval/`:
- `metrics.json` -- per-model `ModelStats` with term breakdowns, plus corpus aggregates.
- `index.html` -- a contact-sheet sorted worst-cost-first; each cell shows the
  best/median/worst renders (and the reference, where present) with their metric
  breakdowns; the header shows corpus geomean and the baseline delta with significance.
- baseline diff -- `compare()` against a small committed `baseline.json`, printed and
  embedded in the report.

The driver declares `required-features = ["png_render", "file_io"]` and is run on demand
(`cargo run --release --example layout_eval`); it is not part of `cargo test`.

### Rung 0 wiring

`select_best_layout` (`mod.rs`) currently keeps the candidate with the fewest crossings
(tie-break on seed). Rung 0 changes it to keep the candidate with the lowest
`weighted_cost` (computed with the accurate crossing count), tie-break on seed. This is the
smallest, immediately-measurable improvement: "best of the candidate seeds" becomes "best
by the full metric." It changes only selection, not the search.

### The iteration loop this enables

Change a parameter or code path -> run the sweep -> read `metrics.json` *and look at the
contact-sheet* -> keep or revert based on the geomean delta and its significance, guarded by
the rendered images. The scalar `weighted_cost` is the hill; the renders are the guardrail
against gaming it (Goodhart); the reference pairs are the objective check that the metric
agrees with human taste.

## Existing Patterns

Investigation grounded every touch point in current code; this design adds pure modules and
one in-tree example, and re-points one existing decision function.

- **Layout module and decision seams.** `src/simlin-engine/src/layout/` holds `mod.rs`
  (orchestration; `count_view_crossings`; `select_best_layout`; `generate_best_layout`
  running the `LAYOUT_SEEDS = [42,123,456,789]` candidates via `par_iter`), `sfdp.rs`
  (force placement, `StdRng::seed_from_u64`), and `annealing.rs` (crossings-only SA cost).
  This design adds `metrics.rs` and `eval_stats.rs` beside them and edits
  `select_best_layout`. Terminology (SFDP, annealing, pinned nodes, chains) follows
  `docs/design-plans/2026-03-27-incremental-layout.md`.
- **Rendering already exists.** `src/simlin-engine/src/diagram/` provides `render.rs`
  (`render_svg`), `render_png.rs` (`render_png` / `svg_to_png`, resvg + embedded
  Roboto-Light, behind the `png_render` feature), with geometry in `elements.rs`,
  `flow.rs`, `connector.rs`, `label.rs` (`label_bounds`), `common.rs` (`Rect`,
  `calc_view_box`), and shared `constants.rs`. The metric reuses these geometry helpers so
  scores match the rendered image -- but only `common`/`constants` are `pub mod` today, so
  the others must be exposed (see Architecture). `render_svg` is asserted byte-identical to
  the TS renderer by `src/diagram/tests/svg-rendering.test.ts`, so the PNG faithfully
  reflects the product UI -- and that test is the tripwire any connector-geometry refactor
  must not break.
- **In-tree example precedent.** `src/simlin-engine/examples/backend_bench.rs` is an
  existing on-demand example (auto-discovered; loads models via `std::fs` +
  `open_vensim`/`open_xmile`). `examples/layout_eval.rs` follows its shape; the
  `required-features` mechanism (used today by the crate's `[[test]]` entries, not by any
  example) means adding a new `[[example]]` block to `Cargo.toml`.
- **Corpus loading.** `tests/layout.rs` loads XMILE via `load_project`/`open_xmile`; its
  DB-backed tests show the salsa-sync-then-layout pattern (`SimlinDb::default()` ->
  `sync_from_datamodel_incremental` -> pass `Some((&mut db, source_project))`). The sweep
  combines that with `open_vensim` for the Vensim `test/metasd` models. (`verify_layout`
  itself is only an assertion helper, not a loader.)
- **Test-time budget.** Per `CLAUDE.md` / `docs/dev/rust.md`, `cargo test --workspace`
  runs under a 3-minute cap and individual tests complete in seconds. The full corpus sweep
  therefore stays in the example (not in tests); only a tiny deterministic guard runs in the
  test suite.
- **FCIS.** Pure cores (`metrics.rs`, `eval_stats.rs`) hold all logic and are unit/property
  tested to the project's coverage bar; the example is a thin imperative shell.

No pattern divergence: pure functions beside existing pure layout code, one example beside
an existing example, one edit to an existing selection function.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Quality-metric core + accurate crossings
**Goal:** A pure, geometry-accurate `LayoutMetrics` and a polyline-based crossing count.

**Components:**
- Expose the `diagram` geometry modules (`elements`, `flow`, `label`, `connector`) as
  `pub(crate)` -- they are private today, so `layout::metrics` cannot call their `*_bounds` /
  path helpers without this.
- `src/simlin-engine/src/layout/metrics.rs` (new) -- `LayoutMetrics`, `MetricWeights`,
  `compute_layout_metrics(view, config)`, `weighted_cost`. Each term computed on the
  `diagram` module's geometry helpers.
- Connector arc-to-polyline geometry factored out of `connector::render_arc` (highest-effort
  item; geometry is currently entangled with SVG-string building), reused by the renderer and
  the metric. The renderer must be re-routed through it without changing its output.
- `count_view_crossings` (`mod.rs`) refactored to count on polylines instead of straight
  chords (Arc/`Link` shapes; flow polylines are already sampled).
- Unit tests on hand-built tiny views with known geometry (two boxes overlapping by a known
  fraction; two segments crossing once; shared-endpoint connectors -> 0; a 1x10 bbox ->
  known aspect penalty; an arc that crosses where its chord would not). Property tests:
  overlap symmetric and scale-invariant; crossings invariant under translation/rotation.

**Dependencies:** none.

**Done when:** the metric terms match the hand-computed values, scale/translation
invariance holds, the polyline crossing count differs from the old chord count on the
constructed arc case, `render_svg` output is unchanged (the `svg-rendering.test.ts` parity
test still passes), and `cargo test` passes. Covers `layout-quality-eval.AC1.*`,
`layout-quality-eval.AC2.*`.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Statistics core
**Goal:** Pure aggregation and significance testing for seed-sample distributions.

**Components:**
- `src/simlin-engine/src/layout/eval_stats.rs` (new) -- `MetricSample`, `ModelStats`,
  `CorpusReport`, `Comparison`; `geomean`, `median`/percentile, and a Mann-Whitney U test;
  `compare(baseline, candidate)` producing per-model and aggregate deltas with p-values.
- Unit tests against known reference values (geomean of a known set; Mann-Whitney U on
  textbook samples; identical baseline/candidate -> zero delta, non-significant).

**Dependencies:** Phase 1 (the `LayoutMetrics` type embedded in `MetricSample`).

**Done when:** the helpers match known values and `compare()` reports the expected
significance verdicts. Covers `layout-quality-eval.AC4.4`, `layout-quality-eval.AC4.5`.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Corpus sweep driver and report
**Goal:** An on-demand sweep that lays out, scores, renders, and reports over the corpus.

**Components:**
- `src/simlin-engine/examples/layout_eval.rs` (new) -- loads a curated corpus list
  (canonical SIR/teacup/logistic-growth; modules; multipoint connectors; LTM/loop models;
  aliases; the `test/ai-information` set; a few large `test/metasd` Vensim models) via
  `open_xmile`/`open_vensim` + salsa sync, runs M seeds per model, scores each, renders
  best/median/worst PNGs, and scores+renders any shipped hand-authored view as a reference.
- The per-seed seam: wrap `generate_layout_with_config` (`mod.rs`) or the `generate` closure
  in `generate_best_layout`, varying `annealing_random_seed` per sample, so the driver can
  sample seeds and compute the best-of-k proxy.
- Emits `metrics.json`, `index.html` contact-sheet, and a `compare()` diff against a
  committed `baseline.json`, under `target/layout-eval/` (gitignored).
- A new `[[example]]` entry in `Cargo.toml` with `required-features = ["png_render",
  "file_io"]` (no example uses `required-features` today; `file_io` helps load Vensim models
  that reference external data, and AC3.6 skip-on-failure covers any that still fail).

**Dependencies:** Phase 1 (metric), Phase 2 (stats).

**Done when:** `cargo run --release --example layout_eval` completes, writes the JSON +
contact-sheet referencing best/median/worst (and reference) renders, reports per-model
median+spread / corpus geomean / best-of-k and a baseline delta with significance, places
artifacts under `target/`, and skips (reports, non-fatally) any model that fails to lay out
or render. Covers `layout-quality-eval.AC3.*`, `layout-quality-eval.AC4.1`,
`layout-quality-eval.AC4.2`, `layout-quality-eval.AC4.3`.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Calibration and reference-pair validation
**Goal:** Commit metric weights that match the user's taste, validated objectively.

**Components:**
- Committed default `MetricWeights` (overlap + crossings dominant; sprawl/aspect moderate;
  structure terms 0), set via a talk-through over the Phase 3 contact-sheet, treating the
  user's "this layout is better than that" judgments as ordering constraints on the linear
  cost.
- A reference-pair fixture (agreed human-vs-AI model pairs, e.g. from `test/ai-information`)
  and a test asserting `weighted_cost(human) < weighted_cost(ai)` under the committed
  weights.

**Dependencies:** Phase 3 (need the contact-sheet to calibrate against), Phase 1.

**Done when:** the committed weights satisfy the reference-pair ordering test, and the user
has signed off on the weights after reviewing the contact-sheet. Covers
`layout-quality-eval.AC5.*`.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Rung 0 wiring + CI regression guard
**Goal:** Make seed selection use the full metric, and protect the gains in normal dev.

**Components:**
- `select_best_layout` (`mod.rs`) re-pointed to minimize `weighted_cost` (accurate
  crossings), tie-break on seed.
- A deterministic regression-guard test over a few tiny models asserting `weighted_cost`
  stays at or below a committed threshold (fixed seeds; fast; under the time budget), plus a
  determinism check (the same seed reproduces a byte-identical layout).
- Confirm existing layout tests (`tests/layout.rs`, `layout_tests.rs`,
  `layout_review_tests.rs`) still pass with the new selection.

**Dependencies:** Phase 1 (metric), Phase 4 (committed weights).

**Done when:** selection picks the lowest-`weighted_cost` candidate (verified on
constructed candidates where lowest-cost differs from fewest-crossings), the guard +
determinism tests pass within budget, and the existing layout suite is green. Covers
`layout-quality-eval.AC6.*`, `layout-quality-eval.AC7.*`, `layout-quality-eval.AC8.1`.
<!-- END_PHASE_5 -->

## Additional Considerations

**The hill-climbing ladder beyond this plan (rungs 1-3).** Rung 0 (Phase 5) is the only
algorithm change built here. The forward path, each rung measured by the Phase 3 sweep with
the Phase 2 significance gate and guarded by the rendered contact-sheet:

- **Rung 1 -- parameter search.** Sweep SFDP `k`, `c`, `p`, the spacing constants, the seed
  count, and SA temperature/iterations (`config.rs`, `sfdp.rs`, `annealing.rs`) against the
  corpus geomean. No algorithm change; pure config search (grid/coordinate descent).
- **Rung 2 -- metric-driven search objective.** Feed `weighted_cost` into the SA acceptance
  delta (`annealing.rs`, currently `perturbed_crossings - current_crossings`) so the search
  optimizes the full metric, not just crossings. Higher leverage but costlier per
  perturbation than a crossing count, so it is a deliberate, measured experiment -- and may
  use a cheap subset of terms in the inner loop.
- **Rung 3 -- new passes.** Targeted code such as an overlap-removal post-pass or
  obstacle-aware connector routing, each validated against the corpus.

**Goodhart guard.** A scalar fitness will be gamed by any optimizer. Three mitigations are
built in: per-term breakdowns stay visible (not just the scalar); the contact-sheet's
best/median/worst renders are inspected every iteration (a change that improves the number
but worsens the picture means the *metric* is wrong, not the layout); and the reference-pair
test fails if weights stop agreeing with human-judged-better layouts.

**Determinism vs. statistical sampling.** These serve different needs. The CI guard uses
fixed seeds (deterministic, fast, flake-free). The interactive sweep varies seeds to
characterize the algorithm's quality distribution, because a single fixed-seed measurement
cannot distinguish a real improvement from seed luck. A specific bad layout remains exactly
reproducible by its seed for debugging.

**Sweep cost.** M seeds x corpus x (layout + a few renders) is minutes-scale on the large
`test/metasd` models; acceptable for an on-demand example, which is why it is not in the
test suite. M and the large-model tier are configurable.

**Metric/render geometry agreement.** Computing the metric from the renderer's own geometry
helpers (rather than the `LayoutConfig` element sizes) guarantees the score reflects what
the PNG shows -- including the connector-polyline sampling that both the renderer and the
crossing count share.
