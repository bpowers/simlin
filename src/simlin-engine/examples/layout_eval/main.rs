// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Layout-quality evaluation sweep (on-demand; NOT part of `cargo test`).
//!
//! For each corpus model: lay it out across many seeds and score each layout
//! with the layout-quality metric (the algorithm's quality DISTRIBUTION), run
//! the production `generate_best_layout` call once, timed (what a user GETS),
//! and render the hand-authored reference, the production layout, and the
//! median and worst seeds to PNG. Writes `metrics.json`, `corpus.json`, and an
//! `index.html` contact sheet under a gitignored `target/` directory.
//!
//! This is a thin imperative shell over the metric core
//! (`layout::metrics::compute_layout_metrics`) and the statistics core
//! (`layout::eval_stats`).
//!
//! Usage:
//!   cargo run --release -p simlin-engine --features png_render,file_io --example layout_eval
//!
//! Env knobs:
//!   LAYOUT_EVAL_MODELS         comma list of corpus keys to run (default: all)
//!   LAYOUT_EVAL_TIERS          comma list of size classes: small,medium,large
//!   LAYOUT_EVAL_EXTRA          comma list of ad-hoc key=path models to add
//!                              (paths relative to src/simlin-engine or absolute)
//!   LAYOUT_EVAL_SEEDS          number of seeds M to sample (default: 25)
//!   LAYOUT_EVAL_OUT            output directory (default: repo-root target/layout-eval)
//!   LAYOUT_EVAL_COMPARE        a previous run's output dir: diff this run against
//!                              its corpus.json (re-scored under this run's weights)
//!                              and show its per-term values beside this run's
//!   LAYOUT_EVAL_WRITE_BASELINE 1 -> write this run's report to the committed
//!                              baseline JSON instead of diffing against it
//!   LAYOUT_EVAL_DECLUTTER      0 -> disable the declutter pass in the seed sweep
//!
//! Baseline diff: the committed `examples/layout_eval_baseline.json` (a
//! serialized `CorpusReport`) records a reference run. A normal run re-scores
//! it under the current weights, compares, and embeds the per-model + paired
//! aggregate verdicts into `metrics.json` and `index.html`.
//!
//! Requires `--features png_render,file_io`: `png_render` for the rasterizer,
//! and `file_io` so Vensim corpus models that reference external data load.

mod corpus;
mod knobs;
mod render;
mod report;
mod sweep;
mod taste;

use simlin_engine::layout::LAYOUT_SEEDS;
use simlin_engine::layout::eval_stats::{Comparison, CorpusReport, ModelStats, compare};
use simlin_engine::layout::metrics::MetricWeights;

use corpus::{ModelSpec, Reference};
use knobs::Knobs;
use report::{ModelFacts, ModelRenders};

/// Path (relative to `CARGO_MANIFEST_DIR`) of the committed baseline report. It
/// lives in the SOURCE TREE by design (checked in and diffed on every normal
/// run), unlike every other artifact, which is written under `target/`.
const BASELINE_REL_PATH: &str = "examples/layout_eval_baseline.json";

fn baseline_path() -> String {
    format!("{}/{}", env!("CARGO_MANIFEST_DIR"), BASELINE_REL_PATH)
}

/// The specs this run lays out: the corpus plus extras, filtered by the
/// `LAYOUT_EVAL_MODELS` keys and `LAYOUT_EVAL_TIERS` classes. Unknown keys are
/// reported so a typo does not silently run nothing.
fn selected_specs(knobs: &Knobs) -> Vec<ModelSpec> {
    let all = corpus::all_specs(&knobs.extra);
    if let Some(keys) = &knobs.models {
        for key in keys {
            if !all.iter().any(|s| &s.key == key) {
                eprintln!("WARN: unknown model key {key:?}; skipping");
            }
        }
    }
    all.into_iter()
        .filter(|s| {
            knobs
                .models
                .as_ref()
                .is_none_or(|keys| keys.contains(&s.key))
        })
        .filter(|s| {
            knobs
                .tiers
                .as_ref()
                .is_none_or(|tiers| tiers.contains(&s.tier))
        })
        .collect()
}

/// One model's pipeline -- load, sweep, production, render -- as the
/// model-level skip-on-failure boundary: ANY failure funnels through the
/// returned `Err`, which `main` WARN-logs before moving on, so one bad model
/// never aborts the sweep. A model that lays out on no seed is a failure; a
/// render that fails is not (its cell is simply empty).
fn process_model(
    spec: &ModelSpec,
    seeds: &[u64],
    knobs: &Knobs,
) -> Result<(ModelStats, ModelRenders, ModelFacts), String> {
    let project = corpus::load_model(spec)?;
    let variables = corpus::variable_count(&project);
    println!("loaded {}: {variables} variables", spec.key);

    let stats = sweep::sweep_model(&spec.key, &project, seeds, knobs.declutter);
    if stats.samples.is_empty() {
        return Err(format!(
            "no usable layout: all {} seed(s) failed to lay out",
            seeds.len()
        ));
    }
    let production = sweep::production(&spec.key, &project);

    let out = &knobs.out;
    let key = &spec.key;
    let reference_view = corpus::reference_view(&project);
    let reference = reference_view.and_then(|sf| {
        render::render_view(
            &project,
            sf,
            None,
            &format!("{key}_reference.png"),
            out,
            true,
        )
    });
    let production_render = production.as_ref().and_then(|p| {
        render::render_view(
            &project,
            &p.view,
            None,
            &format!("{key}_production.png"),
            out,
            true,
        )
    });
    let seed_render = |suffix: &str, seed: u64| {
        sweep::seed_view(key, &project, seed, knobs.declutter).and_then(|view| {
            render::render_view(
                &project,
                &view,
                Some(seed),
                &format!("{key}_{suffix}.png"),
                out,
                false,
            )
        })
    };
    let renders = ModelRenders {
        reference,
        production: production_render,
        median: seed_render("median", stats.median_seed),
        worst: seed_render("worst", stats.worst_seed),
    };

    let (p25, p75) = stats.spread;
    println!(
        "{key}: median={:.4} p25/p75={p25:.4}/{p75:.4} best_of_k={:.4} production={} (M={})",
        stats.median_cost,
        stats.best_of_k_cost,
        match (&production, &renders.production) {
            (Some(p), Some(r)) => format!("{:.4} in {:.0}ms", r.weighted_cost, p.elapsed_ms),
            _ => "n/a".to_string(),
        },
        stats.samples.len(),
    );

    // Taste checks run on the diagrams worth degrading: a single-view reference
    // (a stacked multi-view reference is not one diagram) and production.
    let taste_reference = match (reference_view, spec.reference) {
        (Some(sf), Reference::Curated | Reference::Imported) => taste::run_battery(sf),
        _ => Vec::new(),
    };
    let taste_production = production
        .as_ref()
        .map(|p| taste::run_battery(&p.view))
        .unwrap_or_default();

    let facts = ModelFacts {
        tier: spec.tier,
        reference: if reference_view.is_some() {
            spec.reference
        } else {
            Reference::None
        },
        variables,
        production_ms: production.as_ref().map(|p| p.elapsed_ms),
        taste_reference,
        taste_production,
    };
    Ok((stats, renders, facts))
}

/// Diff `candidate` against the committed baseline (or re-seed it), printing
/// and returning the verdicts. Both sides are scored under `weights`.
fn baseline_comparison(
    candidate: &CorpusReport,
    weights: &MetricWeights,
    knobs: &Knobs,
) -> Option<Comparison> {
    let path = baseline_path();
    if knobs.write_baseline {
        report::write_json(&path, candidate);
        println!("note: re-seed this baseline after the metric terms change.");
        return None;
    }
    let baseline = report::read_corpus(&path)?.rescored(weights, &LAYOUT_SEEDS);
    let cmp = compare(&baseline, candidate);
    report::print_comparison("committed baseline", &cmp);
    Some(cmp)
}

/// Print, per degradation, on how many diagrams the metric penalized it.
fn print_taste_summary(facts: &[ModelFacts]) {
    let Some(first) = facts.iter().find(|f| !f.taste_production.is_empty()) else {
        return;
    };
    println!("taste checks (noticed/applicable): references | production");
    for (i, check) in first.taste_production.iter().enumerate() {
        let (rn, ra) = taste::tally(facts.iter().filter_map(|f| f.taste_reference.get(i)));
        let (pn, pa) = taste::tally(facts.iter().filter_map(|f| f.taste_production.get(i)));
        println!(
            "  {:<12} {rn:>3}/{ra:<3} | {pn:>3}/{pa:<3}",
            check.degradation
        );
    }
}

fn main() {
    let knobs = Knobs::from_env();
    let specs = selected_specs(&knobs);
    let seeds = sweep::seed_set(knobs.seeds);
    std::fs::create_dir_all(&knobs.out)
        .unwrap_or_else(|e| panic!("failed to create output dir {}: {e}", knobs.out));
    println!(
        "layout_eval: {} model(s), M={} seeds (sampling {} unique), out={}",
        specs.len(),
        knobs.seeds,
        seeds.len(),
        knobs.out,
    );

    // `per_model`, `renders`, and `facts` stay positionally paired: all three
    // are pushed exactly once per surviving model.
    let mut per_model = Vec::new();
    let mut renders = Vec::new();
    let mut facts = Vec::new();
    for spec in &specs {
        match process_model(spec, &seeds, &knobs) {
            Ok((stats, model_renders, model_facts)) => {
                per_model.push(stats);
                renders.push(model_renders);
                facts.push(model_facts);
            }
            Err(err) => eprintln!("WARN: skipping {}: {err}", spec.key),
        }
    }

    let weights = MetricWeights::default();
    let corpus = CorpusReport::from_model_stats(per_model);
    println!(
        "corpus: aggregate_cost={:.4} ({} model(s) scored)",
        corpus.aggregate_cost,
        corpus.per_model.len(),
    );

    print_taste_summary(&facts);

    let baseline_cmp = baseline_comparison(&corpus, &weights, &knobs);
    let before_report = knobs
        .compare_dir
        .as_ref()
        .and_then(|dir| report::read_eval_report(&format!("{dir}/metrics.json")));
    let run_cmp = knobs.compare_dir.as_ref().and_then(|dir| {
        let before =
            report::read_corpus(&format!("{dir}/corpus.json"))?.rescored(&weights, &LAYOUT_SEEDS);
        let cmp = compare(&before, &corpus);
        report::print_comparison(&format!("compared run {dir}"), &cmp);
        Some(cmp)
    });

    let eval = report::build_report(
        &corpus.per_model,
        &renders,
        &facts,
        corpus.aggregate_cost,
        &weights,
        baseline_cmp,
        run_cmp,
    );
    let out = &knobs.out;
    report::write_json(&format!("{out}/metrics.json"), &eval);
    report::write_json(&format!("{out}/corpus.json"), &corpus);
    let index_path = format!("{out}/index.html");
    match std::fs::write(
        &index_path,
        report::render_index_html(&eval, before_report.as_ref()),
    ) {
        Ok(()) => println!("wrote {index_path}"),
        Err(err) => eprintln!("WARN: failed to write {index_path}: {err}"),
    }
}
