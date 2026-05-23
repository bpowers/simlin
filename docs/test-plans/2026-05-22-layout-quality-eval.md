# Test Plan: Layout Quality Evaluation

Human verification plan for the layout-quality-eval feature (implementation plan
`docs/implementation-plans/2026-05-22-layout-quality-eval/`). The automated suite
proves the metric math, the selection rule, and per-seed determinism. This plan
covers what automated tests cannot: that the on-demand corpus **sweep** emits the
right artifacts, and that the **human-judgment** calls (best/median/worst
ordering, reference-vs-auto scoring, weight magnitudes) match a modeler's eye.
This is the gate for AC3.*, AC4.1-4.3, and the human-in-the-loop part of AC5.

## Prerequisites

- Repo at a commit including the layout-quality-eval branch, clean working tree.
  Run `./scripts/dev-init.sh`.
- Toolchain that can build `resvg` (the `png_render` feature):
  `cargo build -p simlin-engine --features png_render,file_io --example layout_eval`
  should finish without error.
- A browser to open `target/layout-eval/index.html`, and a JSON viewer / `jq`
  for `target/layout-eval/metrics.json`.
- Automated gate already green:
  `cargo test -p simlin-engine --lib layout::` and
  `cargo test -p simlin-engine --features file_io --test layout`.

## Phase 1: Time-boxed smoke run (fast confidence)

| Step | Action | Expected |
|------|--------|----------|
| 1 | `LAYOUT_EVAL_MODELS=teacup,sir LAYOUT_EVAL_SEEDS=4 cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval` | Exits 0 (AC3.1). stdout prints a per-model `sir: median=… p25/p75=…/… best_of_k=… (M=4)` line and `corpus: geomean_of_medians=… (2 model(s) scored)`. |
| 2 | `ls target/layout-eval/` | Contains `metrics.json`, `index.html`, and PNGs: `sir_best/median/worst/reference.png`, `teacup_best/median/worst/reference.png`. |
| 3 | `git status --porcelain target/` | Empty — nothing under `target/` is tracked (AC3.5). |

## Phase 2: Full corpus sweep + artifact inspection

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval` (no env overrides: all corpus keys, M=25) | Exits 0. Each model prints its median/spread/best-of-k line; corpus aggregate at the end. Runtime is minutes (deliberately kept out of `cargo test`). |
| 2 | Open `target/layout-eval/metrics.json` | Valid JSON. Each `per_model[]` has the full `LayoutMetrics` breakdown (`node_overlap`, `node_connector_overlap`, `label_overlap`, `crossings`, `sprawl`, `edge_length_cv`, `aspect_penalty`, `loop_compactness`, `chain_straightness`) + `weighted_cost`, `median_cost`, `spread`, `best_of_k_cost`, `best/median/worst_seed`. Top level has `geomean_of_medians` and the `weights` set (AC3.2). |
| 3 | Verify AC4.2 by hand: collect each model's `median_cost`, compute their (epsilon-floored) geometric mean, compare to `geomean_of_medians` | The two agree to a few decimals. |
| 4 | Open `target/layout-eval/index.html` in a browser | Contact sheet sorted **worst weighted_cost first**. Each model row shows best/median/worst (and reference where present) thumbnails with a per-term cost breakdown and the `median / p25/p75 / best_of_k / M=25` summary (AC3.3). Header shows `geomean_of_medians` and the weight set. |

## Phase 3: Human-judgment checks (the calibration gate, AC5.1 / AC5.2)

These are the calls only a human can make; sign-off here closes the
human-in-the-loop component of AC5.

| Step | Action | Expected (human judgment) |
|------|--------|---------------------------|
| 1 (best/median/worst ordering) | For 3-4 models (e.g. `sir`, `fishbanks`, `reliability`, `population`), look at the three generated thumbnails side by side | "best" should genuinely look cleanest (fewest overlaps/crossings, labels readable); "worst" messiest. If the metric's "best" looks worse than its "worst", that is calibration feedback — record it, do not silently accept it. |
| 2 (reference vs auto) | For each model shipping a `*_reference.png`, compare it to that model's `*_best.png` and read both `weighted_cost` values | For `reliability`, `fishbanks`, `population`, `logistic-growth`: the hand-authored reference should both look cleaner and carry the lower `weighted_cost` (the human<auto direction the AC5.2 tests pin). For `sir`: the reference deliberately obscures more labels, so the auto scores lower — confirm that asymmetry looks right. |
| 3 (weight magnitudes, AC5.1) | Read the weight set in the `index.html` header / `metrics.json` | Overlap + crossings family carry the dominant weights; `sprawl`/`edge_length_cv`/`aspect_penalty` are 0; `loop_compactness` is a small positive nudge (0.25); `chain_straightness` is 0. Confirm these still match intent over the contact sheet, then sign off. |

## End-to-End: baseline-vs-candidate regression diff (AC4.3)

Validates the full statistical-comparison path (per-model + aggregate deltas with
Mann-Whitney U p-values + significance) a future tuning change would rely on.

1. Seed a baseline: `LAYOUT_EVAL_WRITE_BASELINE=1 cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval`. stdout notes the baseline was written to `examples/layout_eval_baseline.json`.
2. Run a plain candidate sweep (no `WRITE_BASELINE`).
3. In stdout and the `index.html` "baseline diff" section: each model shows a signed `delta_ratio` %, a `p_value`, and a significance verdict; an aggregate delta + verdict is shown.
4. Sanity (matches automated AC4.5): an unchanged candidate vs the just-written baseline shows deltas near 0% and non-significant everywhere. A genuinely different candidate (e.g. after a deliberate weight change) shows non-zero deltas; large, consistent ones read as significant.
5. Reset the committed baseline when done: `git checkout examples/layout_eval_baseline.json` (unless intentionally updating it).

## End-to-End: skip-on-failure (AC3.6)

Confirms one bad model never aborts the sweep.

1. Run a sweep including a model whose file you temporarily make missing/unreadable.
2. Expected: a `WARN: skipping {key}: {err}` line is printed, that model is absent from `metrics.json`/`index.html`, and the sweep still exits 0 and writes a report for the survivors. Restore the file afterward.

## Human Verification Required

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| AC5.1 (weight magnitudes) | Final numeric weights are a taste call over the contact sheet, not derivable from a test. | Phase 3 step 3. |
| AC5.2 (reference-pair selection + sign-off) | Which models are agreed anchors and whether the human layout truly looks better is human judgment. | Phase 3 steps 1-2. |
| AC8.2 (rungs 1-3 documented) | Documentation criterion; no implementation phase. | Read the "Additional Considerations / hill-climbing ladder" of `docs/design-plans/2026-05-22-layout-quality-eval.md`; confirm Rung 1 (`config.rs`/`sfdp.rs`/`annealing.rs`), Rung 2 (`annealing.rs`), Rung 3 (overlap-removal / obstacle-aware routing) are each named with their seam. |

## Notes

- Automated coverage was validated PASS against
  `docs/implementation-plans/2026-05-22-layout-quality-eval/test-requirements.md`
  (20/20 automated criteria; AC3.* and AC4.1-4.3 operational by design; AC8.2 documentation).
- The corpus sweep is intentionally **not** part of `cargo test` (it renders PNGs
  and runs for minutes). It is an on-demand developer tool whose artifacts live
  under the gitignored `target/layout-eval/`.
