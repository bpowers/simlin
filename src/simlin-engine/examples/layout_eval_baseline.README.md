# layout_eval_baseline.json

The committed baseline `CorpusReport` that `examples/layout_eval/` diffs every
normal run against (per-model Mann-Whitney verdicts plus a paired signed-rank
aggregate). A run re-scores the stored per-term metrics under its own weights
before comparing, so a weight change is a pure re-weighting of the same
layouts; a metric TERM added after the baseline was written reads as `0` on
the baseline side until it is re-seeded.

## How this snapshot was seeded

The whole corpus at a reduced seed count, so any model's regression trips the
diff:

```
LAYOUT_EVAL_SEEDS=8 LAYOUT_EVAL_WRITE_BASELINE=1 \
  cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval
```

The sweep takes a few minutes (WRLD3 and covid19 dominate); the JSON is a few
hundred KB.

## When to regenerate

- **When a metric term is added or its definition changes**: the stored term
  values no longer mean what the current metric computes.
- **When the corpus changes**: models missing from either side are skipped by
  the comparison, so a renamed or replaced model silently drops out.
- **After landing an intentional layout-quality improvement**, so the next
  change is measured against it.

Weight-only changes do NOT need a re-seed (the comparison re-scores both
sides).
