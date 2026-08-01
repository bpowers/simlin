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
    /// The cycle partition this period describes (GH #998): dominance is
    /// computed WITHIN a partition, because a loop's importance series is its
    /// share of its own partition's total and is not comparable across
    /// partitions.  `None` labels the shared group of loops that carried no
    /// partition metadata (the layout fallback path).  On the analysis
    /// surface this indexes `ModelAnalysis::partitions`, the same space as
    /// `LoopSummary::partition`.
    pub partition: Option<usize>,
}

/// A feedback loop discovered via LTM analysis.
#[derive(Clone, serde::Serialize)]
pub struct FeedbackLoop {
    pub name: String,
    pub polarity: LoopPolarity,
    pub variables: Vec<String>,
    pub importance_series: Vec<f64>,
    pub dominant_period: Option<DominantPeriod>,
    /// The loop's cycle partition (GH #998), used to group loops for
    /// dominant-period selection: importance is a share WITHIN a partition,
    /// so only partition-mates compete.  `None` when the producing surface
    /// has no partition metadata; all `None` loops share one group.
    pub partition: Option<usize>,
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

/// Calculate dominant periods from feedback loop importance series,
/// PER CYCLE PARTITION (GH #998).
///
/// A loop's importance series is its signed share of its own cycle
/// partition's total absolute loop score, so cross-partition values are not
/// comparable: a loop ALONE in its partition reads exactly `±1` at every
/// active step by construction, and a flat cross-partition ranking is
/// dominated by such groups of one (on C-LEARN the isolated trace-gas decay
/// loops beat every climate loop at every step).  Dominance is therefore
/// computed within each partition independently, and every returned period
/// says which partition it describes.
///
/// Loops with `partition == None` (a surface with no partition metadata,
/// e.g. the layout fallback path) share one group, which preserves the
/// pre-partition behavior exactly for that path.
///
/// Periods are ordered partition-major -- ascending partition index, the
/// `None` group last -- with each partition's periods in time order.
/// Partition indices are dense in first-appearance order over the ranked
/// (competitive-first) loop list, so partition 0 is the most competitive
/// group and leads the output.
///
/// `dt` is the time between consecutive entries in each loop's importance_series.
/// `start_time` is the simulation start time.
pub fn calculate_dominant_periods(
    loops: &[FeedbackLoop],
    start_time: f64,
    dt: f64,
) -> Vec<DominantPeriod> {
    // Group loops by partition, preserving each group's input (ranked)
    // order.  `(is_none, partition)` sorts Some(0), Some(1), ..., None.
    let mut groups: BTreeMap<(bool, usize), Vec<&FeedbackLoop>> = BTreeMap::new();
    for l in loops {
        groups
            .entry((l.partition.is_none(), l.partition.unwrap_or(0)))
            .or_default()
            .push(l);
    }

    groups
        .into_values()
        .flat_map(|group| {
            let partition = group[0].partition;
            calculate_dominant_periods_for_group(&group, partition, start_time, dt)
        })
        .collect()
}

/// The per-partition dominance selection: at each timestep, polarity is
/// determined by score sign (positive = reinforcing, negative = balancing),
/// matching the Praxis reference.  A two-pass approach first computes
/// aggregate totals per polarity, then selects the winning polarity and
/// accumulates loops until the combined score reaches 0.5.  If neither
/// polarity reaches 0.5, all loops from whichever polarity has the higher
/// total are used.  Consecutive timesteps with the same dominant loop set
/// are grouped into a single `DominantPeriod` tagged with `partition`.
fn calculate_dominant_periods_for_group(
    loops: &[&FeedbackLoop],
    partition: Option<usize>,
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
            partition,
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
            partition: None,
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
            partition: None,
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
            partition: None,
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
            partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![-0.3, -0.4, -0.9, -0.9],
                dominant_period: None,
                partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![-0.2, -0.1, -0.7, -0.9],
                dominant_period: None,
                partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "R2".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.20, 0.35, 0.20],
                dominant_period: None,
                partition: None,
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
            partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "A1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.3],
                dominant_period: None,
                partition: None,
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
            partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![-0.3],
                dominant_period: None,
                partition: None,
            },
            FeedbackLoop {
                name: "B2".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["c".to_string()],
                importance_series: vec![-0.25],
                dominant_period: None,
                partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "R2".to_string(),
                polarity: LoopPolarity::Reinforcing,
                variables: vec!["b".to_string()],
                importance_series: vec![0.1],
                dominant_period: None,
                partition: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["c".to_string()],
                importance_series: vec![-0.2],
                dominant_period: None,
                partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["b".to_string()],
                importance_series: vec![-0.5],
                dominant_period: None,
                partition: None,
            },
            FeedbackLoop {
                name: "B2".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["c".to_string()],
                importance_series: vec![-0.4],
                dominant_period: None,
                partition: None,
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
                partition: None,
            },
            FeedbackLoop {
                name: "Z1".to_string(),
                polarity: LoopPolarity::Undetermined,
                variables: vec!["b".to_string()],
                importance_series: vec![0.0],
                dominant_period: None,
                partition: None,
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

    fn partitioned_loop(name: &str, series: Vec<f64>, partition: Option<usize>) -> FeedbackLoop {
        FeedbackLoop {
            name: name.to_string(),
            polarity: LoopPolarity::Undetermined,
            variables: vec![],
            importance_series: series,
            dominant_period: None,
            partition,
        }
    }

    /// The GH #998 shape: a loop ALONE in its partition reads exactly 1.0 at
    /// every step (its share of a one-loop partition is 1 by construction),
    /// while a competitive partition's loops trade dominance.  The old flat
    /// ranking let the singleton smother the competitive partition at every
    /// step; per-partition selection must report BOTH -- the competitive
    /// partition's switch structure AND the singleton's trivial period, each
    /// labeled with its partition.
    #[test]
    fn test_dominant_periods_singleton_partition_does_not_smother() {
        let loops = vec![
            partitioned_loop("R1", vec![0.7, 0.6, 0.1, 0.1], Some(0)),
            partitioned_loop("B1", vec![-0.3, -0.4, -0.9, -0.9], Some(0)),
            // The lone-partition loop: share identically 1.0 while active.
            partitioned_loop("B_lone", vec![-1.0, -1.0, -1.0, -1.0], Some(1)),
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);

        let p0: Vec<_> = periods.iter().filter(|p| p.partition == Some(0)).collect();
        let p1: Vec<_> = periods.iter().filter(|p| p.partition == Some(1)).collect();
        assert_eq!(
            p0.len(),
            2,
            "partition 0 must keep its R1->B1 dominance switch: {periods:?}"
        );
        assert_eq!(p0[0].dominant_loops, vec!["R1"]);
        assert_eq!(p0[1].dominant_loops, vec!["B1"]);
        assert_eq!(
            p1.len(),
            1,
            "the singleton partition reports its own (trivial) period"
        );
        assert_eq!(p1[0].dominant_loops, vec!["B_lone"]);
        assert!(
            !periods
                .iter()
                .any(|p| p.dominant_loops.contains(&"B_lone".to_string())
                    && p.partition != Some(1)),
            "the lone loop must never appear in another partition's periods"
        );
    }

    /// Periods arrive partition-major: ascending partition index first, the
    /// None (no-metadata) group last, times ascending within each partition.
    #[test]
    fn test_dominant_periods_partition_major_ordering() {
        let loops = vec![
            // Deliberately listed out of partition order.
            partitioned_loop("U_meta", vec![0.9, 0.9], None),
            partitioned_loop("R_p1", vec![0.8, 0.8], Some(1)),
            partitioned_loop("R_p0", vec![0.7, 0.7], Some(0)),
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        let order: Vec<Option<usize>> = periods.iter().map(|p| p.partition).collect();
        assert_eq!(
            order,
            vec![Some(0), Some(1), None],
            "periods must be partition-major with the None group last"
        );
        for pair in periods.windows(2) {
            if pair[0].partition == pair[1].partition {
                assert!(pair[0].start <= pair[1].start);
            }
        }
    }

    /// Loops with no partition metadata (the layout fallback path) share ONE
    /// group, so that path's behavior is byte-identical to the pre-partition
    /// flat selection -- including cross-loop accumulation to the 0.5
    /// threshold.
    #[test]
    fn test_dominant_periods_none_partitions_share_one_group() {
        let loops = vec![
            partitioned_loop("R1", vec![0.35], None),
            partitioned_loop("R2", vec![0.20], None),
        ];
        let periods = calculate_dominant_periods(&loops, 0.0, 1.0);
        assert_eq!(periods.len(), 1);
        let mut names = periods[0].dominant_loops.clone();
        names.sort();
        assert_eq!(
            names,
            vec!["R1", "R2"],
            "None-partition loops must accumulate in one shared group"
        );
        assert_eq!(periods[0].partition, None);
    }
}
