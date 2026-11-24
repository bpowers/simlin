// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Lay models out: the per-seed sweep that characterizes the algorithm's
//! quality distribution, and the production call users actually get.

use std::time::Instant;

use rayon::prelude::*;
use simlin_engine::datamodel;
use simlin_engine::layout::config::LayoutConfig;
use simlin_engine::layout::eval_stats::{MetricSample, ModelStats};
use simlin_engine::layout::metrics::{MetricWeights, compute_layout_metrics};
use simlin_engine::layout::{LAYOUT_SEEDS, generate_best_layout, generate_layout_with_config};

use crate::corpus::MAIN_MODEL;

/// The seeds to sample: the union of the production seed set (`LAYOUT_SEEDS`)
/// and `0..m`, deduped and sorted, so the best-of-k production proxy is always
/// computable regardless of `m`.
pub fn seed_set(m: u64) -> Vec<u64> {
    let mut seeds: std::collections::BTreeSet<u64> = (0..m).collect();
    seeds.extend(LAYOUT_SEEDS);
    seeds.into_iter().collect()
}

/// The layout config the sweep uses for one seed.
pub fn seed_config(seed: u64, declutter: bool) -> LayoutConfig {
    LayoutConfig {
        annealing_random_seed: seed,
        declutter,
        ..LayoutConfig::default()
    }
}

/// Lay out `project`'s main model once per seed, score each layout, and
/// summarize the samples.
///
/// The per-seed layouts run in parallel; the results are collapsed back into
/// seed order before summarizing, so every statistic is invariant to rayon's
/// scheduling. `generate_layout_with_config` is deterministic per seed (#633).
/// A seed whose layout fails is dropped with a WARN; a model that fails on
/// every seed yields empty `samples`, which the caller treats as a model-level
/// failure.
pub fn sweep_model(
    key: &str,
    project: &datamodel::Project,
    seeds: &[u64],
    declutter: bool,
) -> ModelStats {
    let mut indexed: Vec<(u64, MetricSample)> = seeds
        .par_iter()
        .filter_map(|&seed| {
            let cfg = seed_config(seed, declutter);
            match generate_layout_with_config(project, MAIN_MODEL, cfg.clone(), None) {
                Ok(view) => {
                    let metrics = compute_layout_metrics(&view, &cfg);
                    let weighted_cost = metrics.weighted_cost(&MetricWeights::default());
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
    let samples = indexed.into_iter().map(|(_, sample)| sample).collect();
    ModelStats::from_samples(key.to_string(), samples, &LAYOUT_SEEDS)
}

/// A regenerated view for one seed (the sweep keeps only scores, so renders
/// regenerate the layouts they draw). `None` (WARN-logged) on failure.
pub fn seed_view(
    key: &str,
    project: &datamodel::Project,
    seed: u64,
    declutter: bool,
) -> Option<datamodel::StockFlow> {
    match generate_layout_with_config(project, MAIN_MODEL, seed_config(seed, declutter), None) {
        Ok(view) => Some(view),
        Err(err) => {
            eprintln!("WARN: {key} seed {seed} failed to lay out: {err}");
            None
        }
    }
}

/// What production hands a user: `generate_best_layout`'s view and how long
/// the call took end to end (metadata extraction, LTM loop detection, and the
/// parallel best-of-k search), in milliseconds.
pub struct Production {
    pub view: datamodel::StockFlow,
    pub elapsed_ms: f64,
}

/// Run the production layout call once, timed. Models run one at a time, so
/// the timing is the call's own wall clock, not contended by other models.
pub fn production(key: &str, project: &datamodel::Project) -> Option<Production> {
    let start = Instant::now();
    match generate_best_layout(project, MAIN_MODEL, None) {
        Ok(view) => Some(Production {
            view,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        }),
        Err(err) => {
            eprintln!("WARN: {key} production layout failed: {err}");
            None
        }
    }
}
