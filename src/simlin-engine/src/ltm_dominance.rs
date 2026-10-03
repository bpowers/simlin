// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Dominant-period selection over LTM loop importance series (GH #998).
//!
//! The post-simulation vocabulary a loop-dominance surface reports --
//! [`FeedbackLoop`] (a loop with its signed partition-relative importance
//! series), [`DominantPeriod`] (a time interval a specific loop set
//! dominates), the coarse 3-way display [`LoopPolarity`] -- plus
//! [`calculate_dominant_periods`], the per-cycle-partition Praxis-style
//! selection, parameterized by the caller-declared [`PartitionSurface`].
//!
//! This is an LTM/analysis module with TWO consumer families:
//! `analysis::analyze_model` (the discovery surface, reaching the FFI /
//! pysimlin / MCP / TS through `ModelAnalysis::dominant_loops_by_period`)
//! and the layout pipeline (`layout::detect_ltm_loops` builds
//! `FeedbackLoop`s to rank loops for diagram placement, and
//! `layout::metadata::ComputedMetadata` carries them). It deliberately sits
//! OUTSIDE `layout`: layout consuming LTM analysis is the right dependency
//! direction; the analysis surface depending on a layout submodule (where
//! these types historically lived) was backwards.

use std::collections::BTreeMap;

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
    /// partitions.  `None` labels a group with no partition index: the
    /// shared group when NO loop carried partition metadata (the layout
    /// fallback path), or one solo loop's own timeline on a
    /// partition-bearing surface (see `calculate_dominant_periods`).  On the
    /// analysis surface this indexes `ModelAnalysis::partitions`, the same
    /// space as `LoopSummary::partition`.
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
    /// has no partition metadata for this loop; see
    /// `calculate_dominant_periods` for how `None` loops are grouped.
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
/// The handling of `partition == None` loops is decided by the CALLER's
/// `surface` declaration, not inferred from the loop list -- an inference
/// ("does any loop carry `Some`?") mislabels a partition-bearing result
/// whose loops are ALL module-internal (every partition legitimately
/// `None`), pooling unrelated `±1`-by-construction series so one smothers
/// the rest (PR #1003 codex review).  On a
/// [`PartitionSurface::PartitionBearing`] surface each `None` loop forms
/// its OWN solo group, mirroring discovery's per-loop `NormGroup::Solo`
/// (GH #750); on [`PartitionSurface::NoMetadata`] (the layout
/// persisted-metadata fallback, where `None` merely means "no partition
/// information exists") all loops share one group, preserving the
/// pre-partition flat selection exactly.
///
/// Periods are ordered partition-major -- ascending partition index, the
/// `None` group(s) last -- with each partition's periods in time order.  On
/// the DISCOVERY surface partition indices are dense in first-appearance
/// order over its ranked (competitive-first) loop list, so there partition 0
/// is the most competitive group and leads the output; the layout
/// detected-loop path derives its indices from the id-sorted detected list
/// instead, where index 0 carries no competitiveness meaning.
///
/// `times[i]` is the time of entry `i` of each loop's importance series: the
/// time of the run's saved row `i`, read from the run, never counted from a
/// cadence (a save step off the DT grid saves rows that are not evenly
/// spaced). Entries past the last time are not read.
///
/// A reported set must reach 50% of the mass its importance series were
/// normalized against (LTM's dominance definition, reference section 2.1).
/// Missing periods mean no supplied set establishes that threshold: the
/// partition may be inactive, or retention/the report cap may have omitted
/// necessary loops. A gap does not establish an absence of feedback. With
/// sample-normalized scores, dominance describes that sample only.
pub fn calculate_dominant_periods(
    loops: &[FeedbackLoop],
    times: &[f64],
    surface: PartitionSurface,
) -> Vec<DominantPeriod> {
    // Group the Some-partition loops by partition, preserving each group's
    // input (ranked) order.
    let mut partitioned: BTreeMap<usize, Vec<&FeedbackLoop>> = BTreeMap::new();
    let mut unpartitioned: Vec<&FeedbackLoop> = Vec::new();
    for l in loops {
        match l.partition {
            Some(p) => partitioned.entry(p).or_default().push(l),
            None => unpartitioned.push(l),
        }
    }

    let none_groups: Vec<Vec<&FeedbackLoop>> = match surface {
        // Each None loop is its own solo group (see the doc above), in
        // input (ranked) order.
        PartitionSurface::PartitionBearing => unpartitioned.into_iter().map(|l| vec![l]).collect(),
        PartitionSurface::NoMetadata if unpartitioned.is_empty() => Vec::new(),
        // One shared group: the exact pre-partition flat selection.
        PartitionSurface::NoMetadata => vec![unpartitioned],
    };

    partitioned
        .into_values()
        .chain(none_groups)
        .flat_map(|group| {
            let partition = group[0].partition;
            calculate_dominant_periods_for_group(&group, partition, times)
        })
        .collect()
}

/// Whether the loops fed to [`calculate_dominant_periods`] come from a
/// surface that carries cycle-partition metadata.  The caller must declare
/// this because the loop list itself cannot: an all-`None` list is EITHER a
/// no-metadata surface (pool everything, the flat legacy selection) or a
/// partition-bearing result whose every loop is module-internal (each is
/// its own competition group).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PartitionSurface {
    /// The loops come from LTM analysis (discovery or detected loops):
    /// `partition == None` means "no parent-level partition", a loop that
    /// competes only against itself.
    PartitionBearing,
    /// The loops carry no partition metadata at all (persisted layout
    /// loop metadata): all loops share one flat dominance group.
    NoMetadata,
}

/// The per-partition dominance selection: at each timestep, polarity is
/// determined by score sign (positive = reinforcing, negative = balancing),
/// matching the Praxis reference.  A two-pass approach first computes
/// aggregate totals per polarity, then selects the winning polarity and
/// accumulates loops until the combined score reaches 0.5. If neither
/// polarity reaches 0.5, the step has no reported dominant set: missing
/// loops could reverse the apparent polarity winner. Consecutive timesteps
/// with the same dominant loop set are grouped into a single `DominantPeriod`
/// tagged with `partition`.
fn calculate_dominant_periods_for_group(
    loops: &[&FeedbackLoop],
    partition: Option<usize>,
    times: &[f64],
) -> Vec<DominantPeriod> {
    if loops.is_empty() {
        return Vec::new();
    }

    // The length of the shortest importance series, and no more steps than
    // there are times.
    let n_steps = loops
        .iter()
        .filter(|l| !l.importance_series.is_empty())
        .map(|l| l.importance_series.len())
        .min()
        .unwrap_or(0)
        .min(times.len());

    if n_steps == 0 {
        return Vec::new();
    }

    let mut periods: Vec<DominantPeriod> = Vec::new();
    let mut score_sum: f64 = 0.0;
    let mut score_count: usize = 0;

    for (step, &time) in times.iter().enumerate().take(n_steps) {
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
        for &(_, score) in &scored {
            if score > 0.0 {
                reinforcing_sum += score;
            } else if score < 0.0 {
                balancing_sum += score.abs();
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
        }

        // Sorted copy for order-independent set comparison
        let mut sorted_names = dominant_names.clone();
        sorted_names.sort();

        // An unresolved step splits periods even if the same set qualifies
        // again later; joining across the gap would assert missing evidence.
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

/// How close to the strongest loop's share another's must be, as a fraction of
/// it, to tie with it at a step. Loops that are equal by construction (the
/// element loops of a symmetric arrayed model) differ in their last bits, and
/// a lead that changed hands on those bits would be a change of lead no model
/// made.
pub const LEAD_TIE: f64 = 1e-9;

/// The share of a group's activity `score` is: its magnitude, nothing where it
/// is not a number.
fn share(score: f64) -> f64 {
    if score.is_finite() { score.abs() } else { 0.0 }
}

/// Which member of a group leads, given each member's share (at a step, or
/// its mean share over a span): the one with the largest, a share within
/// [`LEAD_TIE`] of the largest tying with it and a tie going to the first of
/// the tied in the group's order; `None` when the largest is under `floor`.
/// The one statement of who leads, at a step and over a span alike, so a
/// lead never changes hands between loops that are equal.
pub fn strongest(shares: &[f64], floor: f64) -> Option<usize> {
    let largest = shares.iter().copied().fold(0.0_f64, f64::max);
    if largest < floor || largest <= 0.0 {
        return None;
    }
    shares.iter().position(|&s| s >= largest * (1.0 - LEAD_TIE))
}

/// The loop that leads a group at each of `steps` saved steps
/// ([`strongest`] over the members' shares there; each member of `series` is
/// a loop's partition-relative score series). A series shorter than `steps`
/// holds nothing past its end.
pub fn leader_by_step(series: &[&[f64]], steps: usize, floor: f64) -> Vec<Option<usize>> {
    (0..steps)
        .map(|step| {
            let shares: Vec<f64> = series
                .iter()
                .map(|s| s.get(step).copied().map_or(0.0, share))
                .collect();
            strongest(&shares, floor)
        })
        .collect()
}

/// The mean shares of a group's loops over spans of a run's saved steps:
/// running totals of each loop's share and of the steps at which some loop of
/// the group is active, so a span's mean costs one subtraction per loop
/// whatever its length (a timeline that joins thousands of spans asks for
/// thousands of means). A share is averaged over the span's active steps
/// alone, so the mean shares of a group's loops sum to one, as their shares do
/// at each step; over a span with no active step each is zero. A series
/// shorter than the run holds nothing past its end.
pub struct MeanShares {
    /// `totals[i][k]`: loop `i`'s share summed over the steps before `k`.
    totals: Vec<Vec<f64>>,
    /// `active[k]`: how many of the steps before `k` the group is active at.
    active: Vec<usize>,
}

impl MeanShares {
    pub fn new(series: &[&[f64]], steps: usize) -> MeanShares {
        let mut totals: Vec<Vec<f64>> = series
            .iter()
            .map(|_| {
                let mut total = Vec::with_capacity(steps + 1);
                total.push(0.0);
                total
            })
            .collect();
        let mut active = Vec::with_capacity(steps + 1);
        active.push(0);
        for step in 0..steps {
            let mut any = false;
            for (total, s) in totals.iter_mut().zip(series) {
                let at = s.get(step).copied().map_or(0.0, share);
                any |= at > 0.0;
                let before = total[step];
                total.push(before + at);
            }
            active.push(active[step] + usize::from(any));
        }
        MeanShares { totals, active }
    }

    /// `start..end` within the run.
    fn clamped(&self, start: usize, end: usize) -> (usize, usize) {
        let steps = self.active.len() - 1;
        (start.min(steps), end.clamp(start.min(steps), steps))
    }

    /// How many of the steps `start..end` the group is active at.
    pub fn active(&self, start: usize, end: usize) -> usize {
        let (start, end) = self.clamped(start, end);
        self.active[end] - self.active[start]
    }

    /// Loop `i`'s mean share over the steps `start..end` at which the group
    /// is active.
    pub fn mean(&self, i: usize, start: usize, end: usize) -> f64 {
        let (start, end) = self.clamped(start, end);
        match self.active[end] - self.active[start] {
            0 => 0.0,
            n => (self.totals[i][end] - self.totals[i][start]) / n as f64,
        }
    }

    /// Every loop's mean share over the steps `start..end`, in the group's
    /// order.
    pub fn means(&self, start: usize, end: usize) -> Vec<f64> {
        (0..self.totals.len())
            .map(|i| self.mean(i, start, end))
            .collect()
    }
}

/// A span of a run's saved steps, `start..end`, and the loop that led it
/// (`None` where no loop did).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaderSpan {
    pub start: usize,
    pub end: usize,
    pub leader: Option<usize>,
}

/// A run's dominance timeline: its steps cut where the lead changes
/// (`leaders`, from [`leader_by_step`]), in at most `max_spans` spans.
///
/// The spans are the run-length encoding of `leaders`, so every boundary is a
/// step at which the lead changes. Where that is more than `max_spans` runs,
/// the shortest span (the earliest of equals) joins its longer neighbour (the
/// earlier of equals) until it is not; a span made of several runs is led by
/// `lead(start, end)` (the caller's rule over the span's steps: the loop with
/// the largest mean share, [`strongest`]), and neighbours with one leader are
/// one span. A boundary that survives is still a change of lead: joining
/// only removes boundaries.
///
/// The first step joins the span after it: a score is a change over the step
/// before, so the first saved step has none, which is no span without a
/// leader.
pub fn leader_timeline(
    leaders: &[Option<usize>],
    max_spans: usize,
    lead: impl Fn(usize, usize) -> Option<usize>,
) -> Vec<LeaderSpan> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    /// A span while spans are being joined: a node of a list in step order.
    struct Node {
        start: usize,
        end: usize,
        leader: Option<usize>,
        prev: Option<usize>,
        next: Option<usize>,
        /// Bumped whenever the span changes, so a heap entry made before is
        /// known to be stale; `None` once the span has joined another.
        version: Option<u32>,
    }

    let mut nodes: Vec<Node> = Vec::new();
    for (step, &leader) in leaders.iter().enumerate() {
        // The first step has no score of its own.
        let leader = if step == 0 && leaders.len() > 1 {
            leaders[1]
        } else {
            leader
        };
        match nodes.last_mut() {
            Some(last) if last.leader == leader => last.end = step + 1,
            _ => {
                let prev = nodes.len().checked_sub(1);
                if let Some(prev) = prev {
                    nodes[prev].next = Some(nodes.len());
                }
                nodes.push(Node {
                    start: step,
                    end: step + 1,
                    leader,
                    prev,
                    next: None,
                    version: Some(0),
                });
            }
        }
    }

    let mut live = nodes.len();
    let entry = |nodes: &[Node], i: usize| {
        Reverse((
            nodes[i].end - nodes[i].start,
            nodes[i].start,
            i,
            nodes[i].version.unwrap_or(0),
        ))
    };
    let mut shortest: BinaryHeap<Reverse<(usize, usize, usize, u32)>> =
        (0..nodes.len()).map(|i| entry(&nodes, i)).collect();

    // Join `gone` into its neighbour `kept`: the steps of both, led by
    // `lead` over them.
    let join = |nodes: &mut Vec<Node>, kept: usize, gone: usize| {
        nodes[kept].start = nodes[kept].start.min(nodes[gone].start);
        nodes[kept].end = nodes[kept].end.max(nodes[gone].end);
        nodes[kept].leader = lead(nodes[kept].start, nodes[kept].end);
        let (prev, next) = (nodes[gone].prev, nodes[gone].next);
        if prev == Some(kept) {
            nodes[kept].next = next;
            if let Some(next) = next {
                nodes[next].prev = Some(kept);
            }
        } else {
            nodes[kept].prev = prev;
            if let Some(prev) = prev {
                nodes[prev].next = Some(kept);
            }
        }
        nodes[gone].version = None;
        nodes[kept].version = nodes[kept].version.map(|v| v + 1);
    };

    while live > max_spans.max(1) {
        let Some(Reverse((_, _, i, version))) = shortest.pop() else {
            break;
        };
        if nodes[i].version != Some(version) {
            continue;
        }
        let length = |j: usize| nodes[j].end - nodes[j].start;
        let kept = match (nodes[i].prev, nodes[i].next) {
            (Some(prev), Some(next)) if length(next) > length(prev) => next,
            (Some(prev), _) => prev,
            (None, Some(next)) => next,
            (None, None) => break,
        };
        join(&mut nodes, kept, i);
        live -= 1;
        // Neighbours the join left with one leader are one span.
        loop {
            let same = [nodes[kept].prev, nodes[kept].next]
                .into_iter()
                .flatten()
                .find(|&j| nodes[j].leader == nodes[kept].leader);
            let Some(neighbour) = same else { break };
            join(&mut nodes, kept, neighbour);
            live -= 1;
        }
        shortest.push(entry(&nodes, kept));
    }

    let mut spans = Vec::with_capacity(live);
    let mut at = nodes
        .iter()
        .position(|n| n.version.is_some() && n.prev.is_none());
    while let Some(i) = at {
        spans.push(LeaderSpan {
            start: nodes[i].start,
            end: nodes[i].end,
            leader: nodes[i].leader,
        });
        at = nodes[i].next;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The times of rows saved every `dt` from `start`: more than any series
    /// here holds.
    fn every(start: f64, dt: f64) -> Vec<f64> {
        (0..4096).map(|i| start + i as f64 * dt).collect()
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
    fn test_dominant_periods_empty_loops() {
        let periods =
            calculate_dominant_periods(&[], &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
    fn test_dominant_periods_require_half_the_normalization_mass() {
        // These are valid shares from an incomplete report: the omitted
        // 40% could all be balancing, reversing the apparent 40%-vs-20%
        // winner. No set supplied here establishes the LTM 50% criterion.
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
        assert!(periods.is_empty());
    }

    #[test]
    fn dominance_threshold_and_unresolved_gaps_cover_both_polarities_and_surfaces() {
        // This isolates the threshold/period-grouping decision; the
        // production-derived capped population is covered by
        // ltm_finding::tests::capped_analysis_does_not_reverse_the_dominant_polarity.
        for surface in [
            PartitionSurface::NoMetadata,
            PartitionSurface::PartitionBearing,
        ] {
            for sign in [-1.0, 1.0] {
                let series = vec![0.5, 0.75, 0.0, 0.5, 0.49, 0.5, f64::NAN, 0.5];
                let loops = vec![partitioned_loop(
                    "loop",
                    series.into_iter().map(|v| sign * v).collect(),
                    None,
                )];
                let periods = calculate_dominant_periods(&loops, &every(10.0, 0.25), surface);
                let intervals: Vec<_> = periods.iter().map(|p| (p.start, p.end)).collect();
                assert_eq!(
                    intervals,
                    vec![
                        (10.0, 10.25),
                        (10.75, 10.75),
                        (11.25, 11.25),
                        (11.75, 11.75)
                    ]
                );
                assert_eq!(periods[0].combined_score, 0.625);
                assert!(periods.iter().all(|p| p.combined_score >= 0.5));
            }
        }
    }

    #[test]
    fn equal_polarity_halves_meet_the_threshold() {
        // A 50/50 split has two qualifying sets. The reinforcing-first
        // tie break is deterministic, without requiring a strict majority.
        let loops = vec![
            partitioned_loop("balancing", vec![-0.5], Some(0)),
            partitioned_loop("reinforcing", vec![0.5], Some(0)),
        ];
        let periods = calculate_dominant_periods(
            &loops,
            &every(0.0, 1.0),
            PartitionSurface::PartitionBearing,
        );
        assert_eq!(periods.len(), 1);
        assert_eq!(periods[0].dominant_loops, vec!["reinforcing"]);
        assert_eq!(periods[0].combined_score, 0.5);
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
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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
        // One loop meets the dominance threshold, another has zero score.
        // The zero-score loop contributes nothing and should not inflate
        // the dominant set.
        let loops = vec![
            FeedbackLoop {
                name: "B1".to_string(),
                polarity: LoopPolarity::Balancing,
                variables: vec!["a".to_string()],
                importance_series: vec![-0.5],
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
        let periods = calculate_dominant_periods(
            &loops,
            &every(0.0, 1.0),
            PartitionSurface::PartitionBearing,
        );
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
        let periods = calculate_dominant_periods(
            &loops,
            &every(0.0, 1.0),
            PartitionSurface::PartitionBearing,
        );

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
        let periods = calculate_dominant_periods(
            &loops,
            &every(0.0, 1.0),
            PartitionSurface::PartitionBearing,
        );
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

    /// On a PARTITION-BEARING surface, each `None` loop (no parent-level
    /// partition -- a module-internal loop, whose relative score is +/-1 by
    /// construction) forms its OWN group, mirroring discovery's
    /// `NormGroup::Solo` (GH #750): pooling unrelated `None` loops would let
    /// whichever sorts first smother the rest -- the GH #998 pattern
    /// reappearing inside the `None` subset.
    #[test]
    fn test_none_loops_are_solo_groups_on_a_partitioned_surface() {
        let loops = vec![
            partitioned_loop("R_p0", vec![0.7, 0.7], Some(0)),
            // Two unrelated module-internal loops, both reading +/-1 while
            // active.  Pooled, R_mod would smother B_mod at every step.
            partitioned_loop("R_mod", vec![1.0, 1.0], None),
            partitioned_loop("B_mod", vec![-1.0, -1.0], None),
        ];
        let periods = calculate_dominant_periods(
            &loops,
            &every(0.0, 1.0),
            PartitionSurface::PartitionBearing,
        );

        let none_periods: Vec<_> = periods.iter().filter(|p| p.partition.is_none()).collect();
        assert_eq!(
            none_periods.len(),
            2,
            "each None loop gets its own (solo) timeline: {periods:?}"
        );
        assert!(
            none_periods
                .iter()
                .any(|p| p.dominant_loops == vec!["B_mod"]),
            "B_mod must not be smothered by R_mod: {periods:?}"
        );
        assert!(
            !periods
                .iter()
                .any(|p| p.dominant_loops.len() > 1 && p.partition.is_none()),
            "no pooled None period may exist on a partition-bearing surface"
        );
        // The Some-partition group still leads the output.
        assert_eq!(periods[0].partition, Some(0));
    }

    /// The caller's surface declaration -- not an inference over the loop
    /// list -- decides the `None` handling: a PARTITION-BEARING result whose
    /// loops are ALL module-internal (every partition legitimately `None`)
    /// still gets one solo group per loop.  An "any loop carries Some?"
    /// inference pooled exactly this shape, letting one +/-1 series smother
    /// the rest (PR #1003 codex review).
    #[test]
    fn test_all_none_partition_bearing_loops_stay_solo() {
        let loops = vec![
            partitioned_loop("R_mod", vec![1.0, 1.0], None),
            partitioned_loop("B_mod", vec![-1.0, -1.0], None),
        ];
        let periods = calculate_dominant_periods(
            &loops,
            &every(0.0, 1.0),
            PartitionSurface::PartitionBearing,
        );
        assert_eq!(
            periods.len(),
            2,
            "each all-None loop keeps its own solo timeline: {periods:?}"
        );
        assert!(
            periods.iter().any(|p| p.dominant_loops == vec!["B_mod"]),
            "B_mod must not be smothered by R_mod: {periods:?}"
        );
        assert!(
            periods.iter().all(|p| p.dominant_loops.len() == 1),
            "no pooled period may exist on a partition-bearing surface"
        );
    }

    /// On a NO-METADATA surface (the layout persisted-metadata fallback)
    /// all loops share ONE group, so that path's behavior is byte-identical
    /// to the pre-partition flat selection -- including cross-loop
    /// accumulation to the 0.5 threshold.
    #[test]
    fn test_dominant_periods_none_partitions_share_one_group() {
        let loops = vec![
            partitioned_loop("R1", vec![0.35], None),
            partitioned_loop("R2", vec![0.20], None),
        ];
        let periods =
            calculate_dominant_periods(&loops, &every(0.0, 1.0), PartitionSurface::NoMetadata);
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

    #[test]
    fn the_leader_at_a_step_is_the_loop_with_the_largest_share_there() {
        let a = [0.0, 0.7, -0.2, 0.0005, f64::NAN];
        let b = [0.0, -0.3, 0.8, 0.0, 0.4];
        assert_eq!(
            leader_by_step(&[&a, &b], 5, 0.001),
            [None, Some(0), Some(1), None, Some(1)],
            "by magnitude; none under the floor; a score that is no number holds nothing"
        );
        // A series shorter than the run holds nothing past its end.
        assert_eq!(
            leader_by_step(&[&a[..2], &b], 3, 0.001),
            [None, Some(0), Some(1)]
        );
        assert_eq!(leader_by_step(&[], 2, 0.001), [None, None]);
    }

    #[test]
    fn a_tie_goes_to_the_first_of_the_tied() {
        // Equal to the last bit or two, as the element loops of a symmetric
        // model are: the lead does not change hands on them.
        let third: f64 = 1.0 / 3.0;
        let nudged = f64::from_bits(third.to_bits() + 1);
        let a = [third, nudged, third, 0.2, third];
        let b = [third, third, nudged, 0.6, nudged];
        assert_eq!(
            leader_by_step(&[&a, &b], 5, 0.001),
            [Some(0), Some(0), Some(0), Some(1), Some(0)],
            "the first of the tied, whoever led the step before"
        );
        assert_eq!(strongest(&[0.2, third, nudged], 0.001), Some(1));
        assert_eq!(strongest(&[0.0005, 0.0], 0.001), None, "under the floor");
        assert_eq!(strongest(&[], 0.001), None);
        // A real difference, however small beside the tolerance, is a lead.
        let ahead = third * (1.0 + 1e-6);
        assert_eq!(
            leader_by_step(&[&[third, third], &[third, ahead]], 2, 0.001),
            [Some(0), Some(1)]
        );
    }

    /// The loop that led the most of the steps `start..end` of `leaders`
    /// ([`strongest`] over the fractions of the steps each led): the lead
    /// rule these tests join spans by, where a share is whether a loop led.
    fn most_led(leaders: &[Option<usize>], start: usize, end: usize) -> Option<usize> {
        let loops = leaders.iter().flatten().max().map_or(0, |&m| m + 1);
        let led: Vec<f64> = (0..loops)
            .map(|i| {
                leaders[start..end]
                    .iter()
                    .filter(|&&l| l == Some(i))
                    .count() as f64
            })
            .collect();
        strongest(&led, 0.5)
    }

    fn spans(leaders: &[Option<usize>], max: usize) -> Vec<(usize, usize, Option<usize>)> {
        leader_timeline(leaders, max, |start, end| most_led(leaders, start, end))
            .into_iter()
            .map(|s| (s.start, s.end, s.leader))
            .collect()
    }

    /// `n` steps led by `leader`.
    fn led(leader: usize, n: usize) -> Vec<Option<usize>> {
        vec![Some(leader); n]
    }

    #[test]
    fn a_timeline_is_cut_where_the_lead_changes() {
        let leaders = [led(0, 3), vec![None; 2], led(1, 4)].concat();
        assert_eq!(
            spans(&leaders, 12),
            [(0, 3, Some(0)), (3, 5, None), (5, 9, Some(1))]
        );
        assert_eq!(spans(&[], 12), []);
        assert_eq!(spans(&[None], 12), [(0, 1, None)]);
    }

    #[test]
    fn the_first_step_joins_the_span_after_it() {
        // A score is a change over the step before, so the first step has
        // none: that is not a span no loop led.
        let leaders = [vec![None], led(0, 3), led(1, 2)].concat();
        assert_eq!(spans(&leaders, 12), [(0, 4, Some(0)), (4, 6, Some(1))]);
        // A run no loop leads for longer than its first step is such a span.
        let leaders = [vec![None; 2], led(0, 3)].concat();
        assert_eq!(spans(&leaders, 12), [(0, 2, None), (2, 5, Some(0))]);
    }

    #[test]
    fn the_shortest_spans_join_their_longer_neighbours_past_the_most_a_timeline_has() {
        // Runs of 5, 1, 4, 2, 6 steps.
        let leaders = [led(0, 5), led(1, 1), led(2, 4), led(1, 2), led(0, 6)].concat();
        assert_eq!(spans(&leaders, 5).len(), 5);
        assert_eq!(
            spans(&leaders, 4),
            [
                (0, 6, Some(0)),
                (6, 10, Some(2)),
                (10, 12, Some(1)),
                (12, 18, Some(0))
            ],
            "the one step joins the longer of its neighbours"
        );
        assert_eq!(
            spans(&leaders, 3),
            [(0, 6, Some(0)), (6, 10, Some(2)), (10, 18, Some(0))]
        );
        assert_eq!(spans(&leaders, 1), [(0, 18, Some(0))]);
        assert_eq!(spans(&leaders, 0), [(0, 18, Some(0))], "at least one span");
    }

    #[test]
    fn a_joined_span_is_led_by_the_loop_its_lead_rule_names() {
        // 5 steps of loop 0, then loop 1 for 4, 2 for 4, 1 for 4: joined into
        // one span, loop 1 leads 8 of its 17 steps.
        let leaders = [led(0, 5), led(1, 4), led(2, 4), led(1, 4)].concat();
        assert_eq!(spans(&leaders, 1), [(0, 17, Some(1))]);
        // Spans that come to have one leader are one span.
        let leaders = [led(0, 4), led(1, 1), led(0, 4), led(2, 3)].concat();
        assert_eq!(spans(&leaders, 3), [(0, 9, Some(0)), (9, 12, Some(2))]);
    }

    /// The timeline's rule stated the slow way, to hold the joining against.
    fn timeline_by_rule(leaders: &[Option<usize>], max: usize) -> Vec<LeaderSpan> {
        let mut spans: Vec<LeaderSpan> = Vec::new();
        for step in 0..leaders.len() {
            let leader = if step == 0 && leaders.len() > 1 {
                leaders[1]
            } else {
                leaders[step]
            };
            match spans.last_mut() {
                Some(last) if last.leader == leader => last.end = step + 1,
                _ => spans.push(LeaderSpan {
                    start: step,
                    end: step + 1,
                    leader,
                }),
            }
        }
        while spans.len() > max.max(1) {
            let i = (0..spans.len())
                .min_by_key(|&i| (spans[i].end - spans[i].start, spans[i].start))
                .unwrap();
            let len = |s: &LeaderSpan| s.end - s.start;
            let into_next = match (i.checked_sub(1), spans.get(i + 1)) {
                (Some(prev), Some(next)) => len(next) > len(&spans[prev]),
                (None, Some(_)) => true,
                (Some(_), None) => false,
                (None, None) => unreachable!("more than one span"),
            };
            let mut kept = if into_next { i } else { i - 1 };
            let gone = spans.remove(kept + 1);
            spans[kept].end = gone.end;
            spans[kept].leader = most_led(leaders, spans[kept].start, spans[kept].end);
            loop {
                let leader = spans[kept].leader;
                if kept > 0 && spans[kept - 1].leader == leader {
                    let gone = spans.remove(kept);
                    kept -= 1;
                    spans[kept].end = gone.end;
                } else if spans.get(kept + 1).is_some_and(|s| s.leader == leader) {
                    let gone = spans.remove(kept + 1);
                    spans[kept].end = gone.end;
                } else {
                    break;
                }
                spans[kept].leader = most_led(leaders, spans[kept].start, spans[kept].end);
            }
        }
        spans
    }

    #[test]
    fn a_timeline_keeps_its_rules_whatever_the_leaders() {
        // A fixed stream of leader sequences: runs of random length and
        // leader, some led by no loop.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |n: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % n
        };
        for case in 0..400 {
            let mut leaders: Vec<Option<usize>> = Vec::new();
            for _ in 0..next(30) {
                let leader = match next(5) {
                    4 => None,
                    i => Some(i as usize),
                };
                let length = 1 + next(if case % 2 == 0 { 3 } else { 12 }) as usize;
                leaders.extend(std::iter::repeat_n(leader, length));
            }
            let max = 1 + next(12) as usize;
            let timeline =
                leader_timeline(&leaders, max, |start, end| most_led(&leaders, start, end));
            assert_eq!(
                timeline,
                timeline_by_rule(&leaders, max),
                "{leaders:?} in {max}"
            );
            if leaders.is_empty() {
                assert!(timeline.is_empty());
                continue;
            }
            assert!(timeline.len() <= max, "{leaders:?} in {max}");
            assert_eq!(timeline[0].start, 0);
            assert_eq!(timeline.last().unwrap().end, leaders.len());
            for pair in timeline.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "spans tile the run");
                assert_ne!(pair[0].leader, pair[1].leader, "{leaders:?} in {max}");
                let at = pair[1].start;
                assert!(
                    at == 1 || leaders[at] != leaders[at - 1],
                    "a boundary is a step the lead changes at: {at} of {leaders:?}"
                );
            }
        }
    }

    #[test]
    fn a_mean_share_is_over_the_steps_its_group_is_active_at() {
        let a = [0.0, 0.75, -0.25, 0.0, f64::NAN];
        let b = [0.0, 0.25, 0.75, 0.0, 1.0];
        let shares = MeanShares::new(&[&a, &b], 5);
        assert_eq!(shares.active(0, 5), 3, "steps 1, 2 and 4");
        let (of_a, of_b) = (shares.mean(0, 0, 5), shares.mean(1, 0, 5));
        assert!((of_a - 1.0 / 3.0).abs() < 1e-12, "{of_a}");
        assert!((of_b - 2.0 / 3.0).abs() < 1e-12, "{of_b}");
        assert!((shares.mean(0, 1, 3) - 0.5).abs() < 1e-12);
        assert_eq!(shares.mean(0, 3, 4), 0.0, "no active step");
        assert_eq!(shares.mean(0, 4, 9), 0.0, "past the run, a share no number");
        assert_eq!(shares.means(9, 12), [0.0, 0.0], "wholly past the run");
    }

    /// A timeline joined by mean shares costs in proportion to the run: a
    /// lead that changes hands at every one of a long run's steps leaves tens
    /// of thousands of spans to join, each asking a span's means, which
    /// recounting the span's steps would take minutes over.
    #[test]
    fn a_long_run_that_changes_its_lead_every_step_is_cut_in_proportion_to_its_length() {
        let steps = 100_000;
        let a: Vec<f64> = (0..steps).map(|k| [0.6, 0.4][k % 2]).collect();
        let b: Vec<f64> = (0..steps).map(|k| [0.4, 0.6][k % 2]).collect();
        let series: [&[f64]; 2] = [&a, &b];
        let shares = MeanShares::new(&series, steps);
        let leaders = leader_by_step(&series, steps, 0.001);
        let timeline = leader_timeline(&leaders, 12, |start, end| {
            strongest(&shares.means(start, end), 0.001)
        });
        assert_eq!(timeline.len(), 12);
        assert_eq!((timeline[0].start, timeline[11].end), (0, steps));
    }
}
