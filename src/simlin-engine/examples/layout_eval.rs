// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Layout-quality evaluation sweep (on-demand; NOT part of `cargo test`).
//!
//! Lays out a curated corpus of models across many seeds, scores each layout
//! with the layout-quality metric, renders best/median/worst (and any
//! hand-authored reference) to PNG, and writes a metrics table (JSON), an HTML
//! contact-sheet, and a baseline diff -- all under a gitignored `target/` dir.
//!
//! This is a thin imperative shell over the metric core
//! (`layout::metrics::compute_layout_metrics`) and the statistics core
//! (`layout::eval_stats`). It loads each model via the public `open_xmile` /
//! `open_vensim` loaders (like `examples/backend_bench.rs`), runs
//! `generate_layout_with_config` per seed, scores, summarizes, renders, and
//! emits artifacts.
//!
//! Usage:
//!   cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval
//!   LAYOUT_EVAL_MODELS=teacup,sir cargo run ... --example layout_eval
//!
//! Env knobs:
//!   LAYOUT_EVAL_MODELS  comma list of corpus keys to run (default: all)
//!   LAYOUT_EVAL_SEEDS   number of seeds M to sample (default: 25)
//!   LAYOUT_EVAL_OUT     output directory (default: repo-root target/layout-eval)
//!
//! Requires `--features png_render,file_io`: `png_render` for `render_png`, and
//! `file_io` so Vensim corpus models that reference external data can load.

use std::collections::BTreeSet;
use std::env;
use std::fmt::Write as _;
use std::io::BufReader;

use rayon::prelude::*;
use serde::Serialize;
use simlin_engine::diagram::{PngRenderOpts, render_png};
use simlin_engine::layout::LAYOUT_SEEDS;
use simlin_engine::layout::config::LayoutConfig;
use simlin_engine::layout::eval_stats::{CorpusReport, MetricSample, ModelStats};
use simlin_engine::layout::generate_layout_with_config;
use simlin_engine::layout::metrics::{LayoutMetrics, MetricWeights, compute_layout_metrics};
use simlin_engine::{datamodel, open_vensim, open_xmile};

/// Phase-3 PLACEHOLDER weights for `weighted_cost`.
///
/// `MetricWeights::default()` is all-zeros until Phase 4 commits the calibrated
/// weights (so any accidental pre-calibration use of `weighted_cost` is inert
/// rather than silently wrong). The sweep needs a *non-trivial* scalar to rank
/// seeds (best/median/worst) and to compute the corpus geomean, so this
/// placeholder encodes the design's intended failure-mode priorities:
/// the overlap family (`node_overlap`, `node_connector_overlap`, `label_overlap`)
/// and edge `crossings` are dominant; `sprawl`, `edge_length_cv`, and
/// `aspect_penalty` are moderate; the reserved structure terms
/// (`chain_straightness`, `loop_compactness`, always 0.0 in Phase 1-3) carry
/// zero weight.
///
/// Phase 4 commits the calibrated `MetricWeights` (its `Default`); when it
/// lands, this placeholder MUST be replaced by `MetricWeights::default()` (see
/// the Phase 4 plan, Task 2).
const PLACEHOLDER_WEIGHTS: MetricWeights = MetricWeights {
    node_overlap: 1.0,
    node_connector_overlap: 1.0,
    label_overlap: 1.0,
    crossings: 1.0,
    sprawl: 0.25,
    edge_length_cv: 0.25,
    aspect_penalty: 0.25,
    chain_straightness: 0.0,
    loop_compactness: 0.0,
};

/// The model name the layout pipeline and renderer operate on. `Project::get_model`
/// maps "main" to the single/main model (matching `tests/layout.rs`).
const MAIN_MODEL: &str = "main";

/// Default number of seeds to sample per model when `LAYOUT_EVAL_SEEDS` is unset.
const DEFAULT_SEEDS: u64 = 25;

// ── Corpus ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Format {
    Xmile,
    Vensim,
}

struct ModelSpec {
    key: &'static str,
    /// Path relative to CARGO_MANIFEST_DIR (src/simlin-engine).
    rel_path: &'static str,
    format: Format,
}

use Format::{Vensim, Xmile};

/// The curated corpus. Paths are relative to `CARGO_MANIFEST_DIR`
/// (`src/simlin-engine`); all 15 were verified to exist on disk.
const CORPUS: &[ModelSpec] = &[
    // canonical small
    ModelSpec {
        key: "teacup",
        rel_path: "../../test/test-models/samples/teacup/teacup.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "sir",
        rel_path: "../../test/test-models/samples/SIR/SIR.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "logistic_growth",
        rel_path: "../../test/logistic_growth_ltm/logistic_growth.stmx",
        format: Xmile,
    },
    // modules
    ModelSpec {
        key: "hares_and_foxes",
        rel_path: "../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
        format: Xmile,
    },
    // multipoint connectors
    ModelSpec {
        key: "multipoint",
        rel_path: "../../test/test-models/samples/display/multipoint-connection.stmx",
        format: Xmile,
    },
    // aliases
    ModelSpec {
        key: "alias1",
        rel_path: "../../test/alias1/alias1.stmx",
        format: Xmile,
    },
    // LTM / loop models
    ModelSpec {
        key: "cross_element",
        rel_path: "../../test/cross_element_ltm/cross_element.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "arrayed_pop",
        rel_path: "../../test/arrayed_population_ltm/arrayed_population.stmx",
        format: Xmile,
    },
    // ai-information reference set (human vs AI; used by Phase 4 calibration)
    ModelSpec {
        key: "ai_pure_human",
        rel_path: "../../test/ai-information/PureHumanModel.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "ai_pure_ai",
        rel_path: "../../test/ai-information/PureAIModel.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "ai_edited",
        rel_path: "../../test/ai-information/GeneratedByAIThenEdited.stmx",
        format: Xmile,
    },
    ModelSpec {
        key: "ai_modules_arrays",
        rel_path: "../../test/ai-information/WithModulesAndArrays.stmx",
        format: Xmile,
    },
    // large metasd Vensim
    ModelSpec {
        key: "wrld3_03",
        rel_path: "../../test/metasd/WRLD3-03/wrld3-03.mdl",
        format: Vensim,
    },
    ModelSpec {
        key: "beer_game",
        rel_path: "../../test/metasd/beer-game/RealBeer4-Sterman13.mdl",
        format: Vensim,
    },
    ModelSpec {
        key: "wonderland",
        rel_path: "../../test/metasd/wonderland/Wonderland3.mdl",
        format: Vensim,
    },
];

/// Resolve a corpus-relative path against the crate manifest dir.
fn abs_path(rel: &str) -> String {
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel)
}

/// Load one corpus model, dispatching on its declared format: XMILE through a
/// buffered reader + `open_xmile`, Vensim `.mdl` through a string + `open_vensim`
/// (mirrors `examples/backend_bench.rs`). Returns a human-readable error on any
/// I/O or parse failure so the caller can WARN-and-skip (AC3.6).
fn load_model(spec: &ModelSpec) -> Result<datamodel::Project, String> {
    let path = abs_path(spec.rel_path);
    match spec.format {
        Format::Xmile => {
            let file =
                std::fs::File::open(&path).map_err(|e| format!("failed to open {path}: {e}"))?;
            let mut reader = BufReader::new(file);
            open_xmile(&mut reader).map_err(|e| format!("failed to parse {path}: {e:?}"))
        }
        Format::Vensim => {
            let contents = std::fs::read_to_string(&path)
                .map_err(|e| format!("failed to read {path}: {e}"))?;
            open_vensim(&contents).map_err(|e| format!("failed to parse {path}: {e:?}"))
        }
    }
}

/// Count the view elements in the model's as-loaded main view -- the diagram
/// the later tasks score and render. A model with no hand-authored view yields
/// 0 here (its layout is generated from scratch in Task 2).
fn loaded_element_count(project: &datamodel::Project) -> usize {
    reference_view(project)
        .map(|sf| sf.elements.len())
        .unwrap_or(0)
}

/// Borrow the model's as-loaded main `StockFlow` view if it is a hand-authored
/// reference: a non-empty view carrying non-empty `elements`. A model loaded
/// without a saved diagram (its layout is generated from scratch in the sweep)
/// has no such view, so this returns `None` and the caller skips the reference
/// render.
fn reference_view(project: &datamodel::Project) -> Option<&datamodel::StockFlow> {
    let model = project.get_model(MAIN_MODEL)?;
    match model.views.first() {
        Some(datamodel::View::StockFlow(sf)) if !sf.elements.is_empty() => Some(sf),
        _ => None,
    }
}

// ── Env knobs ────────────────────────────────────────────────────────────────

/// The set of corpus keys to run. `LAYOUT_EVAL_MODELS` is a comma list of keys;
/// unset/empty means the whole corpus. Unknown keys are reported and dropped so
/// a typo does not silently run nothing without explanation.
fn selected_keys() -> Vec<&'static str> {
    let Ok(raw) = env::var("LAYOUT_EVAL_MODELS") else {
        return CORPUS.iter().map(|s| s.key).collect();
    };
    let requested: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if requested.is_empty() {
        return CORPUS.iter().map(|s| s.key).collect();
    }
    let mut keys = Vec::new();
    for want in requested {
        match CORPUS.iter().find(|s| s.key == want) {
            Some(spec) => keys.push(spec.key),
            None => eprintln!("WARN: unknown model key {want:?}; skipping"),
        }
    }
    keys
}

/// Number of seeds M to sample per model (`LAYOUT_EVAL_SEEDS`, default 25).
fn seed_count() -> u64 {
    env::var("LAYOUT_EVAL_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SEEDS)
}

/// The seeds to sample: the union of the production best-of-k proxy
/// (`LAYOUT_SEEDS`) and `0..m`, deduped and sorted. Including `LAYOUT_SEEDS`
/// guarantees the best-of-k production proxy is always computable regardless of
/// `m`.
fn seed_set(m: u64) -> Vec<u64> {
    let mut seeds: BTreeSet<u64> = (0..m).collect();
    seeds.extend(LAYOUT_SEEDS);
    seeds.into_iter().collect()
}

/// The output directory (`LAYOUT_EVAL_OUT`, default repo-root
/// `target/layout-eval`, derived from `CARGO_MANIFEST_DIR`).
fn out_dir() -> String {
    env::var("LAYOUT_EVAL_OUT")
        .unwrap_or_else(|_| format!("{}/../../target/layout-eval", env!("CARGO_MANIFEST_DIR")))
}

// ── Per-model seed sweep ─────────────────────────────────────────────────────

/// Lay out `project`'s main model once for each `seed`, score each layout, and
/// summarize the samples into a `ModelStats`.
///
/// The per-seed layouts run in parallel via rayon (mirroring
/// `generate_best_layout`'s `par_iter` over seeds). The parallel results are
/// collapsed back into `seeds`-order before being summarized, so the sample
/// vector -- and every statistic derived from it -- is invariant to rayon's
/// scheduling: parallelism introduces no nondeterminism here.
///
/// NOTE: `generate_layout_with_config` is itself NOT deterministic per seed --
/// the same `(model, seed)` pair produces slightly different layouts on
/// repeated calls, *even serially within one process* (verified by direct
/// probe). The drift traces to per-process-randomized `HashMap`/`HashSet`
/// iteration order in the layout pipeline (e.g. `sfdp::build_node_index`'s
/// `HashMap` feeding force accumulation). This is a pre-existing layout-engine
/// issue, not a property of this sweep; it means the reported median/spread
/// vary run-to-run within a small band. The fix belongs in the layout engine
/// (deterministic ordered containers). Tracked separately.
///
/// A seed whose layout fails to generate is dropped with a WARN (a single bad
/// seed must not sink the whole model's sweep). The full model-level
/// skip-on-failure path (load/render) lands in a later task; here a model with
/// zero successful seeds yields an empty `ModelStats` (all-zero, no panic).
fn sweep_model(key: &str, project: &datamodel::Project, seeds: &[u64]) -> ModelStats {
    // Compute one (seed, sample) per seed in parallel, then sort back into seed
    // order so the sample vector -- and therefore every statistic derived from
    // it -- is independent of rayon's scheduling.
    let mut indexed: Vec<(u64, MetricSample)> = seeds
        .par_iter()
        .filter_map(|&seed| {
            let cfg = LayoutConfig {
                annealing_random_seed: seed,
                ..LayoutConfig::default()
            };
            match generate_layout_with_config(project, MAIN_MODEL, cfg.clone(), None) {
                Ok(view) => {
                    let metrics = compute_layout_metrics(&view, &cfg);
                    let weighted_cost = metrics.weighted_cost(&PLACEHOLDER_WEIGHTS);
                    Some((
                        seed,
                        MetricSample {
                            seed,
                            metrics,
                            weighted_cost,
                        },
                    ))
                }
                Err(err) => {
                    eprintln!("WARN: {key} seed {seed} failed to lay out: {err}");
                    None
                }
            }
        })
        .collect();

    indexed.sort_by_key(|(seed, _)| *seed);
    let samples: Vec<MetricSample> = indexed.into_iter().map(|(_, sample)| sample).collect();

    ModelStats::from_samples(key.to_string(), samples, &LAYOUT_SEEDS)
}

// ── Rendering ────────────────────────────────────────────────────────────────

/// One rendered diagram: the PNG filename written under the out dir (relative,
/// so the Task-4 `index.html` can reference it with a sibling `<img src>`) and
/// the metric breakdown of the view that was rendered. The seed is `Some` for a
/// generated render (best/median/worst) and `None` for the as-loaded reference.
///
/// `seed`, `metrics`, and `weighted_cost` are read by Task 4: the report builder
/// serializes them into `metrics.json` and the contact-sheet's per-render
/// breakdown table. They are kept as data here (rather than dropped and
/// recomputed) so the report builder is a pure read over this struct.
struct Render {
    /// Filename of the PNG, relative to the out dir (e.g. `sir_best.png`).
    file: String,
    /// The seed that produced the generated view (`None` for the reference).
    seed: Option<u64>,
    /// Per-term metrics of the rendered view.
    metrics: LayoutMetrics,
    /// Scalar weighted cost under the placeholder weights.
    weighted_cost: f64,
}

/// All renders produced for one model: the optional hand-authored reference and
/// the three generated layouts (best/median/worst). Task 4 serializes these
/// per-model metric breakdowns into `metrics.json` and the contact-sheet, so the
/// fields are kept as data the report can read back. A render that failed is
/// `None` (the failure was already WARN-logged) -- skip-on-failure feeds Task 6.
struct ModelRenders {
    reference: Option<Render>,
    best: Option<Render>,
    median: Option<Render>,
    worst: Option<Render>,
}

/// Render one view to a PNG file under `out`, scoring it with the default
/// layout config (the metric core is config-driven only for node sizing, which
/// is constant across the sweep). On any render or write failure, WARN to
/// stderr and return `None` so the sweep continues (AC3.6).
///
/// `project` must already carry the view to render as its main view's first
/// view (the renderer reads `model.views.first()`). The caller installs the
/// view (a clone of the project for a generated layout, or the as-loaded
/// project for the reference) before calling.
fn render_view(
    project: &datamodel::Project,
    metrics: LayoutMetrics,
    seed: Option<u64>,
    file: &str,
    out: &str,
) -> Option<Render> {
    let png = match render_png(project, MAIN_MODEL, &PngRenderOpts::default()) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("WARN: failed to render {file}: {err}");
            return None;
        }
    };
    let path = format!("{out}/{file}");
    if let Err(err) = std::fs::write(&path, &png) {
        eprintln!("WARN: failed to write {path}: {err}");
        return None;
    }
    let weighted_cost = metrics.weighted_cost(&PLACEHOLDER_WEIGHTS);
    Some(Render {
        file: file.to_string(),
        seed,
        metrics,
        weighted_cost,
    })
}

/// Regenerate the view for `seed`, install it into a clone of `project`, render
/// it to `{key}_{suffix}.png`, and return the `Render`. A layout-generation
/// failure is non-fatal: WARN and return `None`.
fn render_generated(
    key: &str,
    suffix: &str,
    project: &datamodel::Project,
    seed: u64,
    out: &str,
) -> Option<Render> {
    let cfg = LayoutConfig {
        annealing_random_seed: seed,
        ..LayoutConfig::default()
    };
    let view = match generate_layout_with_config(project, MAIN_MODEL, cfg.clone(), None) {
        Ok(view) => view,
        Err(err) => {
            eprintln!("WARN: {key} {suffix} (seed {seed}) failed to lay out: {err}");
            return None;
        }
    };
    let metrics = compute_layout_metrics(&view, &cfg);
    // Install the generated view into a clone so the as-loaded project (and its
    // reference view) is never mutated.
    let mut p = project.clone();
    p.get_model_mut(MAIN_MODEL).unwrap().views = vec![datamodel::View::StockFlow(view)];
    let file = format!("{key}_{suffix}.png");
    render_view(&p, metrics, Some(seed), &file, out)
}

/// Render the model's best/median/worst generated layouts and -- if the model
/// ships a hand-authored view -- its reference, all to PNGs under `out`.
///
/// The reference is rendered from the AS-LOADED `project` (before any view is
/// overwritten) so it captures the model's own diagram, not a generated one.
/// Generated layouts are each regenerated from `project` by seed and installed
/// into a fresh clone, leaving `project` untouched.
fn render_model(
    key: &str,
    project: &datamodel::Project,
    stats: &ModelStats,
    out: &str,
) -> ModelRenders {
    // Reference first, from the as-loaded project, before any clone-and-install.
    // Score the hand-authored `StockFlow` directly (the renderer reads the same
    // view from `project`, so this is the geometry being rasterized).
    let reference = reference_view(project).and_then(|sf| {
        let metrics = compute_layout_metrics(sf, &LayoutConfig::default());
        render_view(project, metrics, None, &format!("{key}_reference.png"), out)
    });

    // A model whose sweep produced no samples has all-zero seeds and nothing
    // worth rendering; skip the generated renders (the reference, if any, is
    // already captured).
    if stats.samples.is_empty() {
        return ModelRenders {
            reference,
            best: None,
            median: None,
            worst: None,
        };
    }

    let best = render_generated(key, "best", project, stats.best_seed, out);
    let median = render_generated(key, "median", project, stats.median_seed, out);
    let worst = render_generated(key, "worst", project, stats.worst_seed, out);

    ModelRenders {
        reference,
        best,
        median,
        worst,
    }
}

/// Print the PNG filenames produced for one model (and note a skipped reference
/// or generated render) so a run's stdout records exactly what was written.
fn report_renders(key: &str, renders: &ModelRenders) {
    let mut produced: Vec<&str> = Vec::new();
    for render in [
        &renders.reference,
        &renders.best,
        &renders.median,
        &renders.worst,
    ]
    .into_iter()
    .flatten()
    {
        produced.push(render.file.as_str());
    }
    if produced.is_empty() {
        println!("{key}: no PNGs rendered");
    } else {
        println!("{key}: rendered {}", produced.join(", "));
    }
    if renders.reference.is_none() {
        println!("{key}: no hand-authored reference view (skipped reference render)");
    }
}

// ── Report (metrics.json + index.html) ──────────────────────────────────────
//
// The structs below are the on-disk JSON shape. They are PURE DATA built once
// from the in-memory `ModelStats` + `ModelRenders` the sweep produced, then
// serialized straight to disk -- no recomputation. The contact-sheet HTML is
// rendered from the same `EvalReport`, so the JSON table and the HTML can never
// disagree. Building the report and rendering the HTML are pure (the only I/O
// is the two `std::fs::write` calls in `main`).

/// One rendered view's row in the JSON: the PNG filename, the seed that
/// produced it (`None` for the as-loaded reference), the full per-term
/// `LayoutMetrics` breakdown, and the scalar `weighted_cost` under the weights
/// in use.
#[derive(Serialize)]
struct RenderReport {
    file: String,
    seed: Option<u64>,
    metrics: LayoutMetrics,
    weighted_cost: f64,
}

/// One model's full row in the JSON: its summary statistics (the seed-sweep
/// center/spread, the best-of-k production proxy, the chosen best/median/worst
/// seeds, and `m` -- the number of seeds actually swept) plus each of its
/// renders' per-term breakdowns (`reference` present only when the model ships
/// a hand-authored view).
#[derive(Serialize)]
struct ModelReport {
    model: String,
    /// Number of seeds swept for this model (the union of `LAYOUT_SEEDS` and
    /// `0..M`, deduped). Recorded so a reader can interpret the spread.
    m: usize,
    median_cost: f64,
    /// `(p25, p75)` of the per-seed weighted costs.
    spread: (f64, f64),
    /// Production proxy: min weighted cost over the `LAYOUT_SEEDS` seed set.
    best_of_k_cost: f64,
    best_seed: u64,
    median_seed: u64,
    worst_seed: u64,
    /// The hand-authored reference render + score, when the model ships one.
    reference: Option<RenderReport>,
    best: Option<RenderReport>,
    median: Option<RenderReport>,
    worst: Option<RenderReport>,
}

/// The top-level `metrics.json` document: every scored model plus the corpus
/// aggregates (the geomean of per-model medians and the weight set used).
///
/// `baseline_comparison` is the place Task 5's baseline-vs-candidate diff plugs
/// in (per-model + aggregate deltas with Mann-Whitney p-values). It is `None`
/// here in Phase 3; the field exists so the JSON schema is stable across the
/// two tasks (a Phase-3 reader sees `null`, a Phase-4 reader sees the diff).
#[derive(Serialize)]
struct EvalReport {
    /// Models sorted worst-cost-first (highest `median_cost` at the front), the
    /// same order the contact-sheet renders so the JSON and HTML agree.
    models: Vec<ModelReport>,
    /// Geometric mean of the per-model medians -- the single headline aggregate.
    geomean_of_medians: f64,
    /// The `MetricWeights` used to compute every `weighted_cost` in this report.
    weights: MetricWeights,
    /// Reserved for Task 5's baseline diff; always `None` in Phase 3.
    #[serde(skip_serializing_if = "Option::is_none")]
    baseline_comparison: Option<()>,
}

/// Map an in-memory `Render` to its JSON row.
fn render_report(render: &Render) -> RenderReport {
    RenderReport {
        file: render.file.clone(),
        seed: render.seed,
        metrics: render.metrics,
        weighted_cost: render.weighted_cost,
    }
}

/// Build the serializable report from the sweep's in-memory results.
///
/// PURE: a read over `(per_model, renders)` (paired positionally -- they are
/// pushed together per model in `main`) plus the corpus `geomean_of_medians`
/// and the weight set. Models are sorted worst-cost-first (highest median at
/// the front), the order the contact-sheet inspects top-down as the visual
/// guardrail; ties break on the model name so the order is deterministic.
fn build_report(
    per_model: &[ModelStats],
    renders: &[ModelRenders],
    geomean_of_medians: f64,
    weights: &MetricWeights,
) -> EvalReport {
    let mut models: Vec<ModelReport> = per_model
        .iter()
        .zip(renders.iter())
        .map(|(stats, render)| ModelReport {
            model: stats.model.clone(),
            m: stats.samples.len(),
            median_cost: stats.median_cost,
            spread: stats.spread,
            best_of_k_cost: stats.best_of_k_cost,
            best_seed: stats.best_seed,
            median_seed: stats.median_seed,
            worst_seed: stats.worst_seed,
            reference: render.reference.as_ref().map(render_report),
            best: render.best.as_ref().map(render_report),
            median: render.median.as_ref().map(render_report),
            worst: render.worst.as_ref().map(render_report),
        })
        .collect();

    // Worst-cost-first: highest median at the front. Sort descending by median,
    // tie-break on model name (ascending) for a deterministic ordering. NaN
    // medians can't occur (eval_stats guarantees finite costs), but guard the
    // partial_cmp anyway so a hypothetical NaN never panics the sort.
    models.sort_by(|a, b| {
        b.median_cost
            .partial_cmp(&a.median_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });

    EvalReport {
        models,
        geomean_of_medians,
        weights: *weights,
        baseline_comparison: None,
    }
}

/// HTML-escape the five characters that are special in element text or
/// attribute values. The interpolated strings are static model keys and
/// PNG filenames derived from them, so this is defense-in-depth rather than a
/// live injection vector -- but escaping unconditionally keeps the artifact
/// well-formed if a corpus key ever gains a special character.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Render the per-term metric breakdown for one render as a compact two-column
/// table (term name -> value), with the scalar `weighted_cost` as the final
/// row. PURE: appends to `html`.
fn write_metrics_table(html: &mut String, render: &RenderReport) {
    let m = &render.metrics;
    let rows = [
        ("node_overlap", m.node_overlap),
        ("node_connector_overlap", m.node_connector_overlap),
        ("label_overlap", m.label_overlap),
        ("crossings", m.crossings),
        ("sprawl", m.sprawl),
        ("edge_length_cv", m.edge_length_cv),
        ("aspect_penalty", m.aspect_penalty),
        ("chain_straightness", m.chain_straightness),
        ("loop_compactness", m.loop_compactness),
    ];
    html.push_str("<table class=\"metrics\">");
    for (name, value) in rows {
        let _ = write!(
            html,
            "<tr><td>{name}</td><td class=\"num\">{value:.4}</td></tr>"
        );
    }
    let _ = write!(
        html,
        "<tr class=\"wcost\"><td>weighted_cost</td><td class=\"num\">{:.4}</td></tr>",
        render.weighted_cost
    );
    html.push_str("</table>");
}

/// Render one render's cell (heading + image + breakdown table). A missing
/// render (the model shipped no reference, or its layout/render failed) renders
/// a muted placeholder so the contact-sheet records the gap rather than hiding
/// it. PURE.
fn write_render_cell(html: &mut String, kind: &str, render: Option<&RenderReport>) {
    html.push_str("<div class=\"cell\">");
    let _ = write!(html, "<h4>{}</h4>", html_escape(kind));
    match render {
        Some(r) => {
            let src = html_escape(&r.file);
            let alt = html_escape(&format!("{kind} layout"));
            let _ = write!(html, "<img src=\"{src}\" alt=\"{alt}\">");
            if let Some(seed) = r.seed {
                let _ = write!(html, "<p class=\"seed\">seed {seed}</p>");
            }
            write_metrics_table(html, r);
        }
        None => html.push_str("<p class=\"missing\">(not rendered)</p>"),
    }
    html.push_str("</div>");
}

/// Render the self-contained `index.html` contact-sheet from the report.
///
/// PURE: a string built from `report`. The header shows the corpus
/// `geomean_of_medians` and the weight set; models are laid out one section per
/// model, worst-cost-first (the report is already sorted), each with its
/// reference (if any) and best/median/worst renders side by side and a per-term
/// breakdown under each. `<img>` paths are relative to the out dir so the file
/// references its sibling PNGs.
fn render_index_html(report: &EvalReport) -> String {
    let mut html = String::new();
    html.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Layout quality eval</title>\n<style>\n\
         :root { font-family: Roboto, Helvetica, Arial, sans-serif; }\n\
         body { margin: 24px; color: #1a1a1a; background: #fafafa; }\n\
         h1 { font-size: 20px; margin: 0 0 4px; }\n\
         .summary { color: #555; font-size: 13px; margin-bottom: 16px; }\n\
         .summary code { background: #eee; padding: 1px 4px; border-radius: 4px; }\n\
         table.weights { border-collapse: collapse; font-size: 12px; margin: 8px 0 24px; }\n\
         table.weights td { border: 1px solid #ddd; padding: 2px 8px; }\n\
         .model { border: 1px solid #ddd; border-radius: 4px; background: #fff;\n\
                  padding: 12px 16px; margin-bottom: 20px; }\n\
         .model h2 { font-size: 16px; margin: 0 0 2px; }\n\
         .model .stats { color: #555; font-size: 12px; margin-bottom: 12px; }\n\
         .renders { display: flex; flex-wrap: wrap; gap: 16px; }\n\
         .cell { flex: 0 0 auto; max-width: 280px; }\n\
         .cell h4 { font-size: 13px; margin: 0 0 4px; text-transform: capitalize; }\n\
         .cell img { max-width: 280px; height: auto; border: 1px solid #eee;\n\
                     background: #fff; display: block; }\n\
         .cell .seed { font-size: 11px; color: #888; margin: 4px 0 2px; }\n\
         .cell .missing { font-size: 12px; color: #999; font-style: italic; }\n\
         table.metrics { border-collapse: collapse; font-size: 11px; margin-top: 4px;\n\
                         width: 100%; }\n\
         table.metrics td { border-bottom: 1px solid #f0f0f0; padding: 1px 4px; }\n\
         table.metrics td.num { text-align: right; font-variant-numeric: tabular-nums; }\n\
         table.metrics tr.wcost td { font-weight: 600; border-top: 1px solid #ccc; }\n\
         </style>\n</head>\n<body>\n",
    );

    html.push_str("<h1>Layout quality eval</h1>\n");
    let _ = writeln!(
        &mut html,
        "<p class=\"summary\">Corpus <code>geomean_of_medians = {:.4}</code> over \
         {} model(s), sorted worst-cost-first.</p>",
        report.geomean_of_medians,
        report.models.len(),
    );

    // The weight set used for every weighted_cost in this report.
    let w = &report.weights;
    let weight_rows = [
        ("node_overlap", w.node_overlap),
        ("node_connector_overlap", w.node_connector_overlap),
        ("label_overlap", w.label_overlap),
        ("crossings", w.crossings),
        ("sprawl", w.sprawl),
        ("edge_length_cv", w.edge_length_cv),
        ("aspect_penalty", w.aspect_penalty),
        ("chain_straightness", w.chain_straightness),
        ("loop_compactness", w.loop_compactness),
    ];
    html.push_str("<table class=\"weights\"><caption>weights</caption>");
    for (name, value) in weight_rows {
        let _ = write!(
            &mut html,
            "<tr><td>{name}</td><td class=\"num\">{value:.4}</td></tr>"
        );
    }
    html.push_str("</table>\n");

    for model in &report.models {
        let name = html_escape(&model.model);
        html.push_str("<section class=\"model\">");
        let _ = write!(&mut html, "<h2>{name}</h2>");
        let _ = write!(
            &mut html,
            "<p class=\"stats\">median={:.4} &middot; p25/p75={:.4}/{:.4} &middot; \
             best_of_k={:.4} &middot; M={} &middot; \
             seeds best/median/worst={}/{}/{}</p>",
            model.median_cost,
            model.spread.0,
            model.spread.1,
            model.best_of_k_cost,
            model.m,
            model.best_seed,
            model.median_seed,
            model.worst_seed,
        );
        html.push_str("<div class=\"renders\">");
        write_render_cell(&mut html, "reference", model.reference.as_ref());
        write_render_cell(&mut html, "best", model.best.as_ref());
        write_render_cell(&mut html, "median", model.median.as_ref());
        write_render_cell(&mut html, "worst", model.worst.as_ref());
        html.push_str("</div></section>\n");
    }

    html.push_str("</body>\n</html>\n");
    html
}

fn main() {
    let keys = selected_keys();
    let m = seed_count();
    let seeds = seed_set(m);
    let out = out_dir();

    std::fs::create_dir_all(&out)
        .unwrap_or_else(|e| panic!("failed to create output dir {out}: {e}"));

    let n_sampled = seeds.len();
    println!(
        "layout_eval: {} model(s), M={m} seeds (sampling {n_sampled} unique), out={out}",
        keys.len(),
    );

    let mut per_model: Vec<ModelStats> = Vec::new();
    let mut renders: Vec<ModelRenders> = Vec::new();
    for spec in CORPUS.iter().filter(|s| keys.contains(&s.key)) {
        match load_model(spec) {
            Ok(project) => {
                let n = loaded_element_count(&project);
                println!("loaded {}: {n} elements", spec.key);

                let stats = sweep_model(spec.key, &project, &seeds);
                let (p25, p75) = stats.spread;
                // The actual sampled seed count is the union of LAYOUT_SEEDS and
                // 0..m (deduped), reported here as the real M the run swept.
                println!(
                    "{}: median={:.4} p25/p75={:.4}/{:.4} best_of_k={:.4} (M={n_sampled})",
                    spec.key, stats.median_cost, p25, p75, stats.best_of_k_cost,
                );

                // Render best/median/worst (and the reference, if the model
                // ships one) to PNGs. Render failures are non-fatal.
                let model_renders = render_model(spec.key, &project, &stats, &out);
                report_renders(spec.key, &model_renders);

                per_model.push(stats);
                renders.push(model_renders);
            }
            Err(err) => eprintln!("WARN: skipping {}: {err}", spec.key),
        }
    }

    let corpus = CorpusReport::from_model_stats(per_model);
    println!(
        "corpus: geomean_of_medians={:.4} ({} model(s) scored)",
        corpus.geomean_of_medians,
        corpus.per_model.len(),
    );

    let with_reference = renders.iter().filter(|r| r.reference.is_some()).count();
    println!(
        "corpus: {with_reference}/{} model(s) shipped a hand-authored reference view",
        renders.len(),
    );

    // Build the serializable report from the in-memory stats + renders, then
    // emit both artifacts under the out dir (which defaults under the gitignored
    // repo-root `target/`). `corpus.per_model` and `renders` are positionally
    // paired -- both are pushed once per surviving model in the loop above.
    let report = build_report(
        &corpus.per_model,
        &renders,
        corpus.geomean_of_medians,
        &PLACEHOLDER_WEIGHTS,
    );

    let metrics_path = format!("{out}/metrics.json");
    match serde_json::to_string_pretty(&report) {
        Ok(json) => match std::fs::write(&metrics_path, json) {
            Ok(()) => println!("wrote {metrics_path}"),
            Err(err) => eprintln!("WARN: failed to write {metrics_path}: {err}"),
        },
        Err(err) => eprintln!("WARN: failed to serialize metrics.json: {err}"),
    }

    let index_path = format!("{out}/index.html");
    let html = render_index_html(&report);
    match std::fs::write(&index_path, html) {
        Ok(()) => println!("wrote {index_path}"),
        Err(err) => eprintln!("WARN: failed to write {index_path}: {err}"),
    }
}
