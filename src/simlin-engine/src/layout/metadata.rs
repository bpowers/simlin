// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A stock-flow chain: one or more stocks connected by flows.
#[derive(Clone)]
pub struct StockFlowChain {
    pub stocks: Vec<String>,
    pub flows: Vec<String>,
    pub all_vars: Vec<String>,
    pub importance: f64,
}

/// A time interval during which a specific set of loops dominates behavior.
/// Consecutive timesteps with the same dominant loop set are grouped together.
#[derive(Clone, Debug, PartialEq)]
pub struct DominantPeriod {
    /// Start time of this period.
    pub start: f64,
    /// End time of this period.
    pub end: f64,
    /// Names of the loops that dominate during this period, sorted by score.
    pub dominant_loops: Vec<String>,
    /// Combined relative score of the dominant loops.
    pub combined_score: f64,
}

/// A feedback loop discovered via LTM analysis.
#[derive(Clone)]
pub struct FeedbackLoop {
    pub name: String,
    pub polarity: LoopPolarity,
    pub variables: Vec<String>,
    pub importance_series: Vec<f64>,
    pub dominant_period: Option<DominantPeriod>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoopPolarity {
    Reinforcing,
    Balancing,
    Undetermined,
}

impl FeedbackLoop {
    /// The ordered chain of variable names around the loop.
    pub fn causal_chain(&self) -> &[String] {
        &self.variables
    }

    /// Mean of absolute values of the importance time series.
    pub fn average_importance(&self) -> f64 {
        if self.importance_series.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.importance_series.iter().map(|v| v.abs()).sum();
        sum / self.importance_series.len() as f64
    }
}

/// Pre-computed metadata for driving layout.
#[derive(Clone)]
pub struct ComputedMetadata {
    pub chains: Vec<StockFlowChain>,
    pub feedback_loops: Vec<FeedbackLoop>,
    pub dominant_periods: Vec<DominantPeriod>,
    pub dep_graph: BTreeMap<String, BTreeSet<String>>,
    pub reverse_dep_graph: BTreeMap<String, BTreeSet<String>>,
    pub constants: BTreeSet<String>,
    pub stock_to_inflows: HashMap<String, Vec<String>>,
    pub stock_to_outflows: HashMap<String, Vec<String>>,
    pub flow_to_stocks: HashMap<String, (Option<String>, Option<String>)>,
}

impl ComputedMetadata {
    pub fn new_empty() -> Self {
        Self {
            chains: Vec::new(),
            feedback_loops: Vec::new(),
            dominant_periods: Vec::new(),
            dep_graph: BTreeMap::new(),
            reverse_dep_graph: BTreeMap::new(),
            constants: BTreeSet::new(),
            stock_to_inflows: HashMap::new(),
            stock_to_outflows: HashMap::new(),
            flow_to_stocks: HashMap::new(),
        }
    }

    /// Check if a variable is a constant (no dependencies).
    pub fn is_constant(&self, ident: &str) -> bool {
        self.constants.contains(ident)
    }

    /// Get the stocks connected by a flow: (from_stock, to_stock).
    pub fn connected_stocks(&self, flow_ident: &str) -> (Option<&str>, Option<&str>) {
        self.flow_to_stocks
            .get(flow_ident)
            .map(|(from, to)| (from.as_deref(), to.as_deref()))
            .unwrap_or((None, None))
    }
}

/// Calculate dominant periods from feedback loop importance series.
///
/// At each timestep, loops are sorted by absolute score descending. Loops of
/// the same polarity are accumulated until their combined score >= 0.5 (i.e.,
/// they explain at least half the behavior). Consecutive timesteps with the
/// same dominant loop set are grouped into a single `DominantPeriod`.
///
/// `dt` is the time between consecutive entries in each loop's importance_series.
/// `start_time` is the simulation start time.
pub fn calculate_dominant_periods(
    loops: &[FeedbackLoop],
    start_time: f64,
    dt: f64,
) -> Vec<DominantPeriod> {
    if loops.is_empty() {
        return Vec::new();
    }

    // Find the length of the shortest importance series
    let n_steps = loops
        .iter()
        .filter(|l| !l.importance_series.is_empty())
        .map(|l| l.importance_series.len())
        .min()
        .unwrap_or(0);

    if n_steps == 0 {
        return Vec::new();
    }

    let mut periods: Vec<DominantPeriod> = Vec::new();
    // Track score accumulation for averaging combined_score over a period.
    let mut score_sum: f64 = 0.0;
    let mut score_count: usize = 0;

    for step in 0..n_steps {
        let time = start_time + (step as f64) * dt;

        // Collect (loop_name, score, polarity) for this timestep
        let mut scored: Vec<(&str, f64, LoopPolarity)> = loops
            .iter()
            .filter(|l| step < l.importance_series.len())
            .map(|l| (l.name.as_str(), l.importance_series[step], l.polarity))
            .collect();

        // Sort by absolute score descending
        scored.sort_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Greedily accumulate same-polarity loops until combined >= 0.5
        let mut dominant_names: Vec<String> = Vec::new();
        let mut combined = 0.0_f64;

        if let Some((_, _, lead_polarity)) = scored.first() {
            let lead_polarity = *lead_polarity;
            for &(name, score, polarity) in &scored {
                if polarity != lead_polarity && lead_polarity != LoopPolarity::Undetermined {
                    continue;
                }
                dominant_names.push(name.to_string());
                combined += score.abs();
                if combined >= 0.5 {
                    break;
                }
            }
        }

        // Try to extend the current period or start a new one
        if let Some(last) = periods.last_mut()
            && last.dominant_loops == dominant_names
        {
            last.end = time;
            score_sum += combined;
            score_count += 1;
            continue;
        }

        // Finalize the average for the previous period
        if let Some(last) = periods.last_mut()
            && score_count > 0
        {
            last.combined_score = score_sum / score_count as f64;
        }

        score_sum = combined;
        score_count = 1;
        periods.push(DominantPeriod {
            start: time,
            end: time,
            dominant_loops: dominant_names,
            combined_score: combined,
        });
    }

    // Finalize the last period's average
    if let Some(last) = periods.last_mut()
        && score_count > 0
    {
        last.combined_score = score_sum / score_count as f64;
    }

    periods
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_causal_chain() {
        let fl = FeedbackLoop {
            name: "R1".to_string(),
            polarity: LoopPolarity::Reinforcing,
            variables: vec![
                "population".to_string(),
                "births".to_string(),
                "birth_rate".to_string(),
            ],
            importance_series: vec![],
            dominant_period: None,
        };
        assert_eq!(fl.causal_chain(), &["population", "births", "birth_rate"]);
    }

    #[test]
    fn test_average_importance() {
        let fl = FeedbackLoop {
            name: "B1".to_string(),
            polarity: LoopPolarity::Balancing,
            variables: vec!["a".to_string(), "b".to_string()],
            importance_series: vec![0.5, -0.3, 0.8, -0.4],
            dominant_period: None,
        };
        // abs values: 0.5 + 0.3 + 0.8 + 0.4 = 2.0, mean = 0.5
        let avg = fl.average_importance();
        assert!((avg - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_average_importance_empty() {
        let fl = FeedbackLoop {
            name: "B2".to_string(),
            polarity: LoopPolarity::Undetermined,
            variables: vec![],
            importance_series: vec![],
            dominant_period: None,
        };
        assert!((fl.average_importance() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_is_constant() {
        let mut meta = ComputedMetadata::new_empty();
        meta.constants.insert("gravity".to_string());
        meta.constants.insert("pi".to_string());

        assert!(meta.is_constant("gravity"));
        assert!(meta.is_constant("pi"));
        assert!(!meta.is_constant("population"));
    }

    #[test]
    fn test_connected_stocks() {
        let mut meta = ComputedMetadata::new_empty();
        meta.flow_to_stocks.insert(
            "birth_flow".to_string(),
            (None, Some("population".to_string())),
        );
        meta.flow_to_stocks.insert(
            "transfer".to_string(),
            (Some("source".to_string()), Some("sink".to_string())),
        );

        let (from, to) = meta.connected_stocks("birth_flow");
        assert_eq!(from, None);
        assert_eq!(to, Some("population"));

        let (from, to) = meta.connected_stocks("transfer");
        assert_eq!(from, Some("source"));
        assert_eq!(to, Some("sink"));

        let (from, to) = meta.connected_stocks("nonexistent");
        assert_eq!(from, None);
        assert_eq!(to, None);
    }

    #[test]
    fn test_new_empty_metadata() {
        let meta = ComputedMetadata::new_empty();
        assert!(meta.chains.is_empty());
        assert!(meta.feedback_loops.is_empty());
        assert!(meta.dominant_periods.is_empty());
        assert!(meta.dep_graph.is_empty());
        assert!(meta.reverse_dep_graph.is_empty());
        assert!(meta.constants.is_empty());
        assert!(meta.stock_to_inflows.is_empty());
        assert!(meta.stock_to_outflows.is_empty());
        assert!(meta.flow_to_stocks.is_empty());
    }

    #[test]
    fn test_dominant_periods_empty_loops() {
        let periods = calculate_dominant_periods(&[], 0.0, 1.0);
        assert!(periods.is_empty());
    }

    #[test]
    fn test_dominant_periods_single_dominant_loop() {
        // One loop always dominates (score > 0.5 at every step)
        let loops = vec![FeedbackLoop {
            name: "R1".to_string(),
            polarity: LoopPolarity::Reinforcing,
            variables: vec!["a".to_string(), "b".to_string()],
            importance_series: vec![0.8, 0.7, 0.9],
            dominant_period: None,
        }];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 1);
        assert!((periods[0].start - 0.0).abs() < f64::EPSILON);
        assert!((periods[0].end - 2.0).abs() < f64::EPSILON);
        assert_eq!(periods[0].dominant_loops, vec!["R1"]);
        // combined_score should be the average across all 3 timesteps
        let expected_avg = (0.8 + 0.7 + 0.9) / 3.0;
        assert!(
            (periods[0].combined_score - expected_avg).abs() < 1e-10,
            "combined_score should be average ({expected_avg}), got {}",
            periods[0].combined_score,
        );
    }

    #[test]
    fn test_dominant_periods_switch() {
        // R1 dominates first 2 steps, then B1 takes over
        let loops = vec![
            FeedbackLoop {
                name: "R1".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["a".to_string()],
                importance_series: vec![0.7, 0.6, 0.1, 0.1],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.3, 0.4, 0.9, 0.9],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 2);
        assert_eq!(periods[0].dominant_loops, vec!["R1"]);
        assert_eq!(periods[1].dominant_loops, vec!["B1"]);
    }

    #[test]
    fn test_dominant_periods_combined_score_averaged_across_switch() {
        // R1 dominates steps 0-1 (scores 0.6, 0.8), B1 dominates steps 2-3 (scores 0.7, 0.9)
        let loops = vec![
            FeedbackLoop {
                name: "R1".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["a".to_string()],
                importance_series: vec![0.6, 0.8, 0.1, 0.1],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.2, 0.1, 0.7, 0.9],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 2);

        let r1_avg = (0.6 + 0.8) / 2.0;
        assert!(
            (periods[0].combined_score - r1_avg).abs() < 1e-10,
            "R1 period combined_score should be average ({r1_avg}), got {}",
            periods[0].combined_score,
        );

        let b1_avg = (0.7 + 0.9) / 2.0;
        assert!(
            (periods[1].combined_score - b1_avg).abs() < 1e-10,
            "B1 period combined_score should be average ({b1_avg}), got {}",
            periods[1].combined_score,
        );
    }

    #[test]
    fn test_dominant_periods_no_importance() {
        let loops = vec![FeedbackLoop {
            name: "R1".to_string(),
            polarity: LoopPolarity::Reinforcing,
            variables: vec!["a".to_string()],
            importance_series: vec![],
            dominant_period: None,
        }];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert!(periods.is_empty());
    }
}
