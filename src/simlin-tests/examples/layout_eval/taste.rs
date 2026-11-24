// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Metamorphic taste checks: degrade a good diagram in a way every modeler
//! would call worse, and record whether the metric agrees. See
//! `layout::taste` for the degradations.

use serde::{Deserialize, Serialize};
use simlin_engine::datamodel::StockFlow;
use simlin_engine::layout::config::LayoutConfig;
use simlin_engine::layout::metrics::{LayoutMetrics, MetricWeights, compute_layout_metrics};
use simlin_engine::layout::taste::{Degradation, degrade};

/// The smallest relative cost increase that counts as the metric noticing a
/// degradation. Anything below reads as indifference.
const NOTICE_RATIO: f64 = 0.01;

/// One degradation's outcome on one diagram.
#[derive(Serialize, Deserialize, Clone)]
pub struct TasteCheck {
    pub degradation: String,
    /// `degraded / original - 1`, or `None` when the degradation did not apply.
    pub delta_ratio: Option<f64>,
    /// Whether the metric penalized the degradation by at least `NOTICE_RATIO`.
    pub noticed: Option<bool>,
    /// The degraded diagram's per-term metrics: the data a weight calibration
    /// fits against (every check is a judged pair "original beats degraded").
    #[serde(default)]
    pub degraded_metrics: Option<LayoutMetrics>,
}

/// Every battery degradation applied to `view`.
pub fn run_battery(view: &StockFlow) -> Vec<TasteCheck> {
    let cfg = LayoutConfig::default();
    let weights = MetricWeights::default();
    let base = compute_layout_metrics(view, &cfg).weighted_cost(&weights);
    Degradation::battery()
        .iter()
        .map(|&d| {
            let degraded_metrics =
                degrade(view, d).map(|degraded| compute_layout_metrics(&degraded, &cfg));
            // A zero-cost original has no ratio to speak of: any increase is
            // "noticed", measured against a small floor.
            let delta_ratio =
                degraded_metrics.map(|m| (m.weighted_cost(&weights) - base) / base.max(1e-3));
            TasteCheck {
                degradation: d.name().to_string(),
                delta_ratio,
                noticed: delta_ratio.map(|r| r >= NOTICE_RATIO),
                degraded_metrics,
            }
        })
        .collect()
}

/// `(noticed, applicable)` over a set of checks.
pub fn tally<'a>(checks: impl IntoIterator<Item = &'a TasteCheck>) -> (usize, usize) {
    checks
        .into_iter()
        .filter_map(|c| c.noticed)
        .fold((0, 0), |(n, a), noticed| (n + noticed as usize, a + 1))
}
