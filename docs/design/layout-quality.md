# Layout quality: the metric and the eval harness

Simlin generates stock-and-flow diagrams for models that have none: an agent
building a model over MCP, a notebook user patching a model from Python, an
imported equation file. This document describes how diagram quality is
measured -- the layout-quality metric (`src/simlin-engine/src/layout/metrics.rs`),
its taste checks (`layout/taste.rs`), and the on-demand eval harness
(`src/simlin-engine/examples/layout_eval/`) -- and the loop for improving the
layout algorithm against them.

## The metric

`compute_layout_metrics(view)` scores a diagram as a vector of terms, each `0`
when ideal, and `weighted_cost` collapses them with `MetricWeights::default()`.
`generate_best_layout` picks the cheapest of several seeds, so the metric is
production code, not just an evaluation tool.

### The scene

Every term is computed over the geometry the renderer actually draws: node
shapes at their drawn size (a flow valve is the 9px circle `render_flow` draws),
flow pipes as 4px-thick segments, links as the exact polylines
`diagram::connector` draws (arcs sampled along their circle), and labels at the
boxes `diagram::label` measures. The declutter pass (`layout/declutter.rs`)
avoids the same obstacles the metric charges: it pushes apart the footprints
the metric finds overlapping, and chooses each label's side by the metric's own
charge for that label (`metrics::LabelScene`), so what the optimizer removes is
what the score counts.

### Terms

The defect terms are RATES (means over the nodes, labels, or connectors they
concern). A model's cost therefore does not grow with its size, one defect costs
`weight / n` in a model of `n` things, the trade-off between terms is the same
for a 10-variable model as for a 300-variable one, and the corpus aggregate is
not dominated by the largest models.

| term | measures | weight |
|---|---|---|
| `node_overlap` | mean covered fraction of each node's shape by other shapes | 3.5 |
| `label_overlap` | mean covered fraction of each label by other labels and shapes | 3.5 |
| `node_connector_overlap` | fraction of connector length under non-incident shapes or pipes (a false causal link) | 2.0 |
| `label_connector_overlap` | mean over labels of connector length -- links and other flows' pipes -- through the text, relative to the box's smaller side; a node's own links count half | 1.5 |
| `crossings` | connector crossings per connector | 1.0 |
| `crowding` | clearance deficits `(1 - gap/8px)^2` between non-cloud footprints per node, plus links too short to show their arrow per link | 1.0 |
| `long_connectors` | mean excess of links beyond 3x the median link length | 0.5 |
| `sprawl` | mean connector length over characteristic node size | 0.25 |
| `loop_compactness` | isoperimetric penalty of feedback-loop polygons | 0.4 |
| `flow_bends` | bends per flow pipe | 0.15 |
| `misalignment` | fraction of nodes sharing no row or column with a nearby node | 0.1 |
| `loop_straightness` | bow shortfall of loop connectors | 0.1 |
| `edge_length_cv`, `aspect_penalty` | reported, unweighted | 0 |

`crowding` and `sprawl` pull in opposite directions and together set a finite
optimum spacing: spread until neighbors have air, no further. A flow and the
stock or cloud its pipe attaches to are joined by construction, so their SHAPES
being close is exempt from crowding and from the declutter's overlap check --
their labels are not.

### Defects

`analyze_layout(view)` returns the metrics together with every defect behind
them, located on the diagram. It runs the same code as
`compute_layout_metrics` with a recording sink, so an overlay drawn from the
defects can never disagree with the score.

### Calibration

A weight is only as good as the judgments it reproduces. The weights are
checked against two kinds of judged pairs:

- **Taste checks.** `layout::taste::degrade` applies edits every modeler would
  call regressions -- crowd the diagram (`Cramp`), spread it (`Inflate`),
  scatter free nodes (`Jitter`, `Shuffle`), park the most-used parameter across
  the diagram (`Exile`), drop one node on another (`Stack`), flatten curved
  links (`StraightenLinks`) -- moving only what a person drags by hand and
  keeping each link's bow. The metric must charge each. The unit test
  `test_metric_penalizes_every_degradation_of_the_exemplars` pins this on the
  shipped default projects; the harness runs the battery over every corpus
  reference and production layout.
- **Visual judgments.** Reference-versus-production pairs where a person, looking
  at the renders, finds one clearly better. The reference-pair unit tests pin
  the default projects' hand-drawn diagrams beating a generated layout.

The committed weights are rounded priors that a log-space fit against those
pairs (squared hinge on a 2% margin, anchored at `crossings = 1`) moved by under
15%. Judgments the terms cannot reproduce at any weights point at missing
terms, not at weights -- today that is structural taste: chains laid out in
rows, parameters beside their consumers, the arrangement that makes a
hand-drawn diagram read as organized rather than merely uncluttered.
`StraightenLinks` is deliberately not required: flattening a non-loop link is
style, and only loops care.

## The eval harness

```
cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval
```

Knobs (environment variables): `LAYOUT_EVAL_MODELS` (corpus keys),
`LAYOUT_EVAL_TIERS` (`small,medium,large`), `LAYOUT_EVAL_EXTRA` (`key=path` ad
hoc models), `LAYOUT_EVAL_SEEDS` (default 25), `LAYOUT_EVAL_OUT` (default
`target/layout-eval`), `LAYOUT_EVAL_COMPARE` (a previous run's output dir),
`LAYOUT_EVAL_WRITE_BASELINE`, `LAYOUT_EVAL_DECLUTTER=0`,
`LAYOUT_EVAL_REPLAY_STEPS` (0 skips the replay).

For each corpus model it:

- sweeps the seeds, scoring each layout (the algorithm's quality distribution,
  summarized benchstat-style in `layout::eval_stats`);
- runs `generate_best_layout` once, timed -- the layout a user gets and what it
  costs;
- replays building the model from empty over a few edits
  (`LAYOUT_EVAL_REPLAY_STEPS`, default 4) -- each stock-flow chain arriving
  whole, then the other variables nearest the backbone first -- syncing the
  diagram after every edit the way MCP `edit_model` and pysimlin's patch sync
  do (`generate_best_layout` while the view is empty, `incremental_layout`
  after), and scores the final diagram: what an agent or notebook user ends up
  looking at, which can differ sharply from a fresh layout because incremental
  layout preserves everything already placed;
- renders the hand-drawn reference, the production and incremental layouts,
  and the median and worst seeds to PNG (small diagrams upscaled so labels are
  legible), with `*_defects.png` overlays for the reference, production, and
  incremental diagrams and a `*.view.json` of every rendered view;
- runs the taste battery on the reference and the production layout.

It writes `metrics.json` (per-term breakdowns, timings, taste checks),
`corpus.json` (per-seed samples), and `index.html` (a contact sheet plus the
corpus-wide taste matrix).

### The corpus and its references

`examples/layout_eval/corpus.rs` grades how far each model's shipped diagram can
be trusted:

- **Curated**: one view authored in Stella or Simlin, whose drawing conventions
  the renderer reproduces; its score is directly comparable to a generated
  layout's.
- **Imported**: one Vensim view. Its arrangement is a trustworthy exemplar, but
  Vensim draws a variable as its wrapped name, so the label geometry our renderer
  imposes is not what the author saw; label-dependent terms over it are not
  comparable.
- **MultiView**: several views the importer stacks into one diagram with group
  boxes -- an exemplar of decomposing a large model, not one comparable diagram.
- **None**: no shipped diagram.

The corpus spans textbook models, the default projects, AI-built models, module
and array models, and large published models, in three size tiers.

### Comparing runs

`LAYOUT_EVAL_COMPARE=<dir>` diffs a run against an earlier run's `corpus.json`,
re-scored under the current weights: per-model Mann-Whitney verdicts over the
seed samples, and a paired Wilcoxon signed-rank test over the models' shifted-log
median ratios for the aggregate. The contact sheet shows the earlier run's term
values beside the current ones. The committed baseline
(`examples/layout_eval_baseline.json`, see its README) is diffed the same way on
every run.

## Evaluating edits

A diagram an agent or a notebook user edits is synced after every patch by
`incremental_layout`. Its quality is not one static score: a sync must leave
alone what the edit did not touch, keep the view consistent with the model, and
put what it creates somewhere sensible. `layout::edit_audit` states that
contract from an edit's inputs and outputs alone, in three layers:

- **Scope.** An untouched element comes back exactly as it was. Touched is
  derived from the two models and the patch: a deleted variable, a renamed one
  (only its name changes), one whose kind changed (rebuilt, keeping its center
  unless it became a flow or its new shape there would cover another shape), a flow whose attachment changed. A link whose
  dependency survives keeps its uid, endpoints, polarity, and shape, and a link
  drawing no dependency the model has survives unless the patch names its
  reader. A connector the view did not draw is drawn only into a variable the
  patch names or between elements drawn for the first time, and a variable the
  view did not draw is drawn only when the patch names it: what an author left
  out elsewhere stays out.
- **Consistency.** Every variable drawn once with its kind, references resolve,
  links and drawn dependencies agree, flows attach where the stock lists say,
  and every flow the sync created or changed holds the strict flow invariants
  (`editing::invariants`). Only findings the edit introduced count, so an
  imported view's own inconsistencies are not charged to an edit.
- **Placement.** What the sync created or changed does not cover another shape
  it did not already cover before the edit, and a pipe it routed does not pass
  through a stock that is not one of its ends.

It also records what it does not charge: how far rebuilt elements moved, and
the metric's cost before and after.

`layout::edit_scenarios` generates the edits for any model -- restate a
variable, add or delete a parameter, insert an intermediate, delete a flow or a
middle stock, detach a flow, turn an aux into a stock, rename (with the rename
operation, and the way an agent without one does it), add a flow between two
stocks, close a loop, extend a chain, add a side flow, add a sector, add then
undo -- picking targets deterministically, and runs each through `apply_patch`
and the production sync rule, auditing every step and checking that two syncs
of one edit agree and that an edit expected to return the original view does.

The unit battery (`layout/edit_scenarios_tests.rs`) drives every scenario over
hand-drawn and imported views and pins every finding in `KNOWN_DEFECTS`, one row
per (fixture, scenario, finding) naming the defect: a finding no row expects
fails, and so does a row that no longer reproduces. The harness runs the same
scenarios over the whole corpus (`LAYOUT_EVAL_EDITS=0` skips them) and writes
`edits.json` and `edits.html`, with each run's last step rendered before (removed
elements marked) and after (created, changed, and every located finding marked).

## The improvement loop

1. Run the harness on the current code into one directory, and on the changed
   code into another with `LAYOUT_EVAL_COMPARE` pointing at the first.
2. Read the verdicts and per-term deltas, and the production timings.
3. Look at the production renders and their defect overlays. A defect the eye
   finds that no mark covers is a blind spot in the metric; a mark over
   something that reads fine is a false positive. Either means the metric needs
   work before its number can be trusted.
4. Keep a change only when the numbers improve AND the pictures do. A change
   that improves the number while the picture gets worse means the metric is
   wrong, not the diagram.
5. After landing an improvement, re-seed the committed baseline.

The hand-drawn references are ground truth for arrangement. A generated layout
scoring better than an exemplary Curated reference is a prompt to look, not a
win: either the generator truly beat the author, or the metric is missing what
the author got right.
