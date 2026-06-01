# layout_eval_baseline.json

The committed baseline `CorpusReport` that `examples/layout_eval.rs` diffs every
normal run against (per-model + aggregate deltas with Mann-Whitney U p-values).

## How this snapshot was seeded

This baseline covers the **whole corpus** (including the large metasd Vensim
models) at a reduced seed count, so any model's regression trips the diff:

```
LAYOUT_EVAL_SEEDS=8 LAYOUT_EVAL_WRITE_BASELINE=1 \
  cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval
```

It records the **layout behavior at the start of the layout-hill-climb work**
(post the corpus expansion to multi-view Vensim models and the switch of the
corpus aggregate to the shifted geomean `aggregate_cost`), scored with the
committed calibrated `MetricWeights::default()`. Re-seeded on 2026-05-31.

The sweep is minutes-scale (the wrld3/covid19 models dominate); the committed
JSON is ~100-200KB. Both are acceptable for a tripwire that is regenerated
rarely and diffed on every eval run.

## When to regenerate

REGENERATE this baseline:

- **Whenever the calibrated `MetricWeights::default()` change**: the weighted
  costs change, so the recorded sample costs go stale.
- **Whenever the corpus aggregate definition changes** (e.g. the
  `geomean_of_medians` -> `aggregate_cost` switch): the old JSON no longer
  deserializes, and a normal run will WARN and skip the diff until re-seeded.
- **After landing an intentional layout-quality improvement**: re-seed so the
  baseline reflects the new behavior and the next change is measured against
  it (each rung of the hill-climb re-seeds after it lands).

Re-run the seeding command above and commit the regenerated
`layout_eval_baseline.json`.
