// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A stock-flow chain: one or more stocks connected by flows.
#[derive(Clone, serde::Serialize)]
pub struct StockFlowChain {
    pub stocks: Vec<String>,
    pub flows: Vec<String>,
    pub all_vars: Vec<String>,
    pub importance: f64,
}

/// A time interval during which a specific set of loops dominates behavior.
/// Consecutive timesteps with the same dominant loop set are grouped together.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
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
#[derive(Clone, serde::Serialize)]
pub struct FeedbackLoop {
    pub name: String,
    pub polarity: LoopPolarity,
    pub variables: Vec<String>,
    pub importance_series: Vec<f64>,
    pub dominant_period: Option<DominantPeriod>,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
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
#[derive(Clone, serde::Serialize)]
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
/// At each timestep, polarity is determined by score sign (positive =
/// reinforcing, negative = balancing), matching the Praxis reference.
/// A two-pass approach first computes aggregate totals per polarity,
/// then selects the winning polarity and accumulates loops until the
/// combined score reaches 0.5. If neither polarity reaches 0.5, all
/// loops from whichever polarity has the higher total are used.
///
/// Consecutive timesteps with the same dominant loop set are grouped
/// into a single `DominantPeriod`.
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
    let mut score_sum: f64 = 0.0;
    let mut score_count: usize = 0;

    for step in 0..n_steps {
        let time = start_time + (step as f64) * dt;

        // Collect (loop_name, score) for this timestep
        let mut scored: Vec<(&str, f64)> = loops
            .iter()
            .filter(|l| step < l.importance_series.len())
            .map(|l| (l.name.as_str(), l.importance_series[step]))
            .collect();

        // Sort by absolute score descending
        scored.sort_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Pass 1: compute polarity totals using score sign
        let mut reinforcing_sum = 0.0_f64;
        let mut balancing_sum = 0.0_f64;
        let mut reinforcing_loops: Vec<&str> = Vec::new();
        let mut balancing_loops: Vec<&str> = Vec::new();

        for &(name, score) in &scored {
            if score > 0.0 {
                reinforcing_sum += score;
                reinforcing_loops.push(name);
            } else if score < 0.0 {
                balancing_sum += score.abs();
                balancing_loops.push(name);
            }
        }

        // Pass 2: select dominant loops from the winning polarity.
        // Compare totals first so the larger polarity always wins,
        // even when both exceed the 0.5 threshold.
        let mut dominant_names: Vec<String> = Vec::new();
        let mut combined = 0.0_f64;

        let reinforcing_wins = reinforcing_sum >= balancing_sum;
        let winning_sum = if reinforcing_wins {
            reinforcing_sum
        } else {
            balancing_sum
        };

        if winning_sum >= 0.5 {
            // Accumulate loops from winning polarity until cumulative >= 0.5
            for &(name, score) in &scored {
                let dominated = if reinforcing_wins {
                    score > 0.0
                } else {
                    score < 0.0
                };
                if dominated {
                    dominant_names.push(name.to_string());
                    combined += score.abs();
                    if combined >= 0.5 {
                        break;
                    }
                }
            }
        } else if reinforcing_wins {
            // Fallback: use ALL loops from the higher-scoring polarity
            dominant_names = reinforcing_loops.iter().map(|s| s.to_string()).collect();
            combined = reinforcing_sum;
        } else {
            dominant_names = balancing_loops.iter().map(|s| s.to_string()).collect();
            combined = balancing_sum;
        }

        // Sorted copy for order-independent set comparison
        let mut sorted_names = dominant_names.clone();
        sorted_names.sort();

        // Skip timesteps with no meaningful dominance
        if combined == 0.0 {
            if let Some(last) = periods.last_mut()
                && score_count > 0
            {
                last.combined_score = score_sum / score_count as f64;
            }
            score_sum = 0.0;
            score_count = 0;
            continue;
        }

        // Try to extend the current period if the dominant set matches
        if score_count > 0
            && let Some(last) = periods.last_mut()
        {
            let mut last_sorted = last.dominant_loops.clone();
            last_sorted.sort();
            if last_sorted == sorted_names {
                last.end = time;
                score_sum += combined;
                score_count += 1;
                continue;
            }
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
        // R1 dominates first 2 steps (positive scores), then B1 takes over
        // (negative scores indicate balancing behavior).
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
                importance_series: vec![-0.3, -0.4, -0.9, -0.9],
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
        // R1 dominates steps 0-1 (positive scores 0.6, 0.8),
        // B1 dominates steps 2-3 (negative scores -0.7, -0.9)
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
                importance_series: vec![-0.2, -0.1, -0.7, -0.9],
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
    fn test_dominant_periods_same_set_different_order() {
        // Both R1 and R2 are needed to reach 0.5 at every timestep, but
        // their relative scores swap between steps. The dominant *set*
        // is the same so this should produce a single period, not two.
        let loops = vec![
            FeedbackLoop {
                name: "R1".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["a".to_string()],
                importance_series: vec![0.35, 0.20, 0.35],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "R2".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.20, 0.35, 0.20],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(
            periods.len(),
            1,
            "same dominant set with swapped order should produce one period, got {:?}",
            periods
                .iter()
                .map(|p| &p.dominant_loops)
                .collect::<Vec<_>>(),
        );
        // Both loops should appear in the dominant set, ordered by score
        // (R1 has the higher score at the first timestep)
        let mut names = periods[0].dominant_loops.clone();
        names.sort();
        assert_eq!(names, vec!["R1", "R2"]);
    }

    #[test]
    fn test_dominant_periods_split_across_zero_gap() {
        // R1 dominates at steps 0, 1, then has zero score at step 2,
        // then dominates again at steps 3, 4. This should produce two
        // separate periods, not one continuous period bridging the gap.
        let loops = vec![FeedbackLoop {
            name: "R1".to_string(),
            polarity: LoopPolarity::Reinforcing,
            variables: vec!["a".to_string()],
            importance_series: vec![0.8, 0.7, 0.0, 0.9, 0.6],
            dominant_period: None,
        }];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(
            periods.len(),
            2,
            "zero-score gap should split into two periods, got {:?}",
            periods.iter().map(|p| (p.start, p.end)).collect::<Vec<_>>(),
        );
        assert!((periods[0].start - 0.0).abs() < f64::EPSILON);
        assert!((periods[0].end - 1.0).abs() < f64::EPSILON);
        assert!((periods[1].start - 3.0).abs() < f64::EPSILON);
        assert!((periods[1].end - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_dominant_periods_score_ordering_preserved() {
        // Verify that dominant_loops preserves score-based ordering,
        // not alphabetical.
        let loops = vec![
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["a".to_string()],
                importance_series: vec![0.6],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "A1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.3],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 1);
        // B1 has the higher score so should come first despite being
        // alphabetically after A1.
        assert_eq!(periods[0].dominant_loops[0], "B1");
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

    #[test]
    fn test_dominant_periods_aggregate_polarity_wins_over_leader() {
        // The leading loop (highest abs score) is reinforcing (+0.4), but
        // the aggregate balancing total (0.3 + 0.25 = 0.55) exceeds 0.5
        // while the reinforcing total (0.4) does not. The balancing loops
        // should dominate, not the leading reinforcing loop.
        let loops = vec![
            FeedbackLoop {
                name: "R1".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["a".to_string()],
                importance_series: vec![0.4],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![-0.3],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "B2".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["c".to_string()],
                importance_series: vec![-0.25],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 1);
        // R1 should NOT be in the dominant set
        assert!(
            !periods[0].dominant_loops.contains(&"R1".to_string()),
            "reinforcing loop should not dominate when balancing aggregate exceeds 0.5: {:?}",
            periods[0].dominant_loops,
        );
        // Both balancing loops should appear
        assert!(periods[0].dominant_loops.contains(&"B1".to_string()));
        assert!(periods[0].dominant_loops.contains(&"B2".to_string()));
    }

    #[test]
    fn test_dominant_periods_fallback_uses_all_from_higher_polarity() {
        // Neither polarity reaches 0.5, so all loops from the polarity
        // with the higher aggregate total should be used.
        let loops = vec![
            FeedbackLoop {
                name: "R1".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["a".to_string()],
                importance_series: vec![0.3],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "R2".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.1],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["c".to_string()],
                importance_series: vec![-0.2],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 1);
        // Reinforcing total (0.4) > Balancing total (0.2), so both R1+R2
        let mut names = periods[0].dominant_loops.clone();
        names.sort();
        assert_eq!(names, vec!["R1", "R2"]);
    }

    #[test]
    fn test_dominant_periods_picks_larger_polarity_when_both_exceed_threshold() {
        // Both polarity totals exceed 0.5, but balancing has the larger
        // aggregate. The winning polarity should be balancing, not
        // reinforcing.
        let loops = vec![
            FeedbackLoop {
                name: "R1".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["a".to_string()],
                importance_series: vec![0.6],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![-0.5],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "B2".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["c".to_string()],
                importance_series: vec![-0.4],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 1);
        // Balancing total (0.9) > reinforcing total (0.6), so balancing
        // should win even though reinforcing also exceeds 0.5.
        assert!(
            !periods[0].dominant_loops.contains(&"R1".to_string()),
            "reinforcing should not dominate when balancing has larger total: {:?}",
            periods[0].dominant_loops,
        );
        assert!(
            periods[0].dominant_loops.contains(&"B1".to_string()),
            "B1 should be in dominant set: {:?}",
            periods[0].dominant_loops,
        );
    }

    #[test]
    fn test_dominant_periods_zero_score_loops_excluded_from_dominant_set() {
        // One loop has a small negative score, another has zero score.
        // The zero-score loop contributes nothing and should not inflate
        // the dominant set in the fallback path.
        let loops = vec![
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["a".to_string()],
                importance_series: vec![-0.01],
                dominant_period: None,
            },
            FeedbackLoop {
                name: "Z1".to_string(),
                polarity: LoopPolarity::Undetermined,
                variables: vec!["b".to_string()],
                importance_series: vec![0.0],
                dominant_period: None,
            },
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 1);
        assert_eq!(
            periods[0].dominant_loops,
            vec!["B1"],
            "zero-score loop Z1 should not appear in dominant set, got: {:?}",
            periods[0].dominant_loops,
        );
    }
}
