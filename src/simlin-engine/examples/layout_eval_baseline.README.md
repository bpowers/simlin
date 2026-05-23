# layout_eval_baseline.json

The committed baseline `CorpusReport` that `examples/layout_eval.rs` diffs every
normal run against (per-model + aggregate deltas with Mann-Whitney U p-values).

## How this snapshot was seeded

This baseline was seeded over a **small representative subset** of the corpus to
keep the run fast and the committed JSON modest:

```
LAYOUT_EVAL_MODELS=sir,teacup LAYOUT_EVAL_SEEDS=8 LAYOUT_EVAL_WRITE_BASELINE=1 \
  cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval
```

It records the **current pre-Rung-0 layout behavior**, scored with the committed
calibrated `MetricWeights::default()`. It was re-seeded on 2026-05-23 after
Phase 4 committed those weights and `layout_eval.rs` switched from the Phase-3
`PLACEHOLDER_WEIGHTS` to `MetricWeights::default()`. Do not seed the full metasd
corpus here: that is minutes-scale and produces a large JSON.

## When to regenerate

REGENERATE this baseline:

- **Whenever the calibrated `MetricWeights::default()` change**: the weighted
  costs change, so the recorded sample costs go stale.
- **Before Phase 5 measures Rung 0's improvement**: the baseline must capture
  pre-Rung-0 behavior with the final calibrated weights so the Rung-0 diff is
  meaningful.

Re-run the seeding command above (optionally over a broader model set / larger
`LAYOUT_EVAL_SEEDS`) and commit the regenerated `layout_eval_baseline.json`.
