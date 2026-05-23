// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// Pure statistics for layout-quality seed-sample distributions, mirroring Go's
// `benchstat`: many per-seed samples reduced to a center + spread, plus a
// non-parametric significance test (Mann-Whitney U) on differences.
//
// There is NO I/O in this module: it takes slices of numbers, computes scalars,
// and returns them. Every primitive returns a finite, documented default
// (`0.0`, or a non-significant `p_value` of `1.0`) on empty or degenerate
// input -- it must never return NaN, matching the engine's no-NaN policy for
// statistics. That makes every term trivially testable with hand-computed
// expected values (see the inline tests below).
//
// The corpus sweep (Phase 3) is the imperative shell that fills these structs
// from real layouts.

use crate::layout::metrics::LayoutMetrics;

/// Geometric mean of strictly-positive values: `exp(mean(ln(x)))`.
///
/// Returns `0.0` for an empty slice. Values must be `> 0`; layout costs are
/// `>= 0`, so callers floor with a small epsilon before calling (see
/// [`CorpusReport::from_model_stats`]) so a single `0` cost cannot zero the
/// whole-corpus geometric mean.
pub fn geomean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    // The geometric mean of a single value is that value exactly; short-circuit
    // to avoid a needless ln/exp round-trip (which would return e.g.
    // 4.999999999999999 for an input of 5.0).
    if values.len() == 1 {
        return values[0];
    }
    let sum_ln: f64 = values.iter().map(|&x| x.ln()).sum();
    (sum_ln / values.len() as f64).exp()
}

/// Linear-interpolated percentile using the "type 7" convention (NumPy's
/// default): for sorted `x` of length `n` and `p` in `[0, 1]`, the fractional
/// rank is `p * (n - 1)`, then the result interpolates linearly between the
/// values at the floor and ceil of that rank.
///
/// Returns `0.0` for an empty slice and the single value for `n == 1`.
/// `values` need not be pre-sorted -- a copy is sorted internally. `p` is
/// clamped to `[0, 1]`.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let n = values.len();
    if n == 1 {
        return values[0];
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p = p.clamp(0.0, 1.0);
    // Type-7 fractional rank in [0, n-1].
    let rank = p * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return sorted[lo];
    }
    let frac = rank - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// Median, equal to `percentile(values, 0.5)`.
pub fn median(values: &[f64]) -> f64 {
    percentile(values, 0.5)
}

/// Mann-Whitney U test result for two independent samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MannWhitney {
    /// The smaller of `u1` and `u2`.
    pub u: f64,
    /// U statistic for sample `a`.
    pub u1: f64,
    /// U statistic for sample `b`.
    pub u2: f64,
    /// Two-sided p-value (normal approximation with tie + continuity
    /// correction).
    pub p_value: f64,
}

/// Mann-Whitney U (a.k.a. Wilcoxon rank-sum) test on two independent samples.
///
/// Ranks the pooled samples, averaging tied ranks; computes U from the rank
/// sums; reports the two-sided p-value via the normal approximation with tie
/// correction and continuity correction. For tiny samples this approximation
/// is rough; the sweep uses M >= ~20 seeds where it is good.
///
/// Returns `p_value = 1.0` (non-significant) when either sample is empty or all
/// pooled values are identical (no separation is possible, so the variance of
/// the normal approximation is zero).
pub fn mann_whitney_u(a: &[f64], b: &[f64]) -> MannWhitney {
    let n1 = a.len();
    let n2 = b.len();
    if n1 == 0 || n2 == 0 {
        // No separation possible with an empty sample. Report a degenerate but
        // finite result with a non-significant p-value.
        return MannWhitney {
            u: 0.0,
            u1: 0.0,
            u2: 0.0,
            p_value: 1.0,
        };
    }

    // 1. Pool, tagging each value with which sample it came from (false = a),
    //    sort by value, and assign average ranks (1..=N) to tied groups.
    let mut pooled: Vec<(f64, bool)> = Vec::with_capacity(n1 + n2);
    pooled.extend(a.iter().map(|&v| (v, false)));
    pooled.extend(b.iter().map(|&v| (v, true)));
    pooled.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    let n = (n1 + n2) as f64;
    let mut r1 = 0.0; // sum of ranks belonging to sample `a`
    // Σ (t^3 - t) over each tie group of size t, for the variance correction.
    let mut tie_term = 0.0;
    let mut i = 0;
    while i < pooled.len() {
        // Extend [i, j) over the run of values equal to pooled[i].0.
        let mut j = i + 1;
        while j < pooled.len() && pooled[j].0 == pooled[i].0 {
            j += 1;
        }
        let group_len = j - i;
        // Ranks are 1-based; the average rank of positions i..j (0-based) is
        // ((i+1) + j) / 2.
        let avg_rank = ((i + 1) + j) as f64 / 2.0;
        for entry in &pooled[i..j] {
            if !entry.1 {
                r1 += avg_rank;
            }
        }
        if group_len > 1 {
            let t = group_len as f64;
            tie_term += t * t * t - t;
        }
        i = j;
    }

    // 2. U statistics from the rank sums.
    let n1f = n1 as f64;
    let n2f = n2 as f64;
    let u1 = r1 - n1f * (n1f + 1.0) / 2.0;
    let u2 = n1f * n2f - u1;
    let u = u1.min(u2);

    // 3. Mean and tie-corrected variance of the U distribution.
    let mu = n1f * n2f / 2.0;
    let variance = (n1f * n2f / 12.0) * ((n + 1.0) - tie_term / (n * (n - 1.0)));

    // 4. Two-sided p-value via the normal approximation with a 0.5 continuity
    //    correction. When the variance is zero (all pooled values identical,
    //    or n == 1 with no spread), no separation is possible -- report the
    //    non-significant default rather than dividing by zero.
    let p_value = if variance <= 0.0 {
        1.0
    } else {
        let z = ((u - mu).abs() - 0.5).max(0.0) / variance.sqrt();
        (2.0 * (1.0 - phi(z))).clamp(0.0, 1.0)
    };

    MannWhitney { u, u1, u2, p_value }
}

/// Error function via the Abramowitz & Stegun 7.1.26 rational approximation
/// (max absolute error ~1.5e-7) -- ample accuracy for a significance verdict.
///
/// A small local copy keeps this module self-contained and independently
/// testable (the VM-internal `crate::alloc::erfc_approx`/`normal_cdf` are an
/// implementation detail of the allocation opcodes).
fn erf(x: f64) -> f64 {
    // A&S 7.1.26 is stated for x >= 0; erf is odd, so reflect for x < 0.
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();

    const A1: f64 = 0.254_829_592;
    const A2: f64 = -0.284_496_736;
    const A3: f64 = 1.421_413_741;
    const A4: f64 = -1.453_152_027;
    const A5: f64 = 1.061_405_429;
    const P: f64 = 0.327_591_1;

    let t = 1.0 / (1.0 + P * x);
    // Horner form of (a1 t + a2 t^2 + a3 t^3 + a4 t^4 + a5 t^5).
    let poly = ((((A5 * t + A4) * t + A3) * t + A2) * t + A1) * t;
    let y = 1.0 - poly * (-x * x).exp();
    sign * y
}

/// Standard normal CDF, `Phi(x) = 0.5 * (1 + erf(x / sqrt(2)))`.
fn phi(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// Floor applied to each model's median before it enters the corpus geometric
/// mean. A geometric mean is the product of its terms, so a single `0` median
/// would zero the whole aggregate; flooring with this small epsilon keeps a
/// genuinely-perfect (zero-cost) model from collapsing the corpus number while
/// remaining far below any meaningful cost. Documented and applied only in
/// [`CorpusReport::from_model_stats`].
pub const GEOMEAN_FLOOR_EPSILON: f64 = 1e-9;

/// One per-seed layout sample: the seed that produced the layout, its computed
/// metrics, and the scalar weighted cost the optimizer minimizes.
#[derive(Clone, Debug)]
pub struct MetricSample {
    pub seed: u64,
    pub metrics: LayoutMetrics,
    pub weighted_cost: f64,
}

/// Aggregated statistics for one model's seed sweep: the raw per-seed samples
/// plus the center (`median_cost`), spread (`p25`, `p75`), the best-of-k
/// production proxy, and the best/median/worst seeds (which drive Phase 3's
/// PNG renders).
#[derive(Clone, Debug)]
pub struct ModelStats {
    pub model: String,
    /// One sample per seed.
    pub samples: Vec<MetricSample>,
    pub median_cost: f64,
    /// `(p25, p75)` of the weighted costs.
    pub spread: (f64, f64),
    /// Production proxy: the min weighted cost over the k production seeds.
    pub best_of_k_cost: f64,
    pub best_seed: u64,
    pub median_seed: u64,
    pub worst_seed: u64,
}

/// Corpus-wide report: one `ModelStats` per model plus the geometric mean of
/// the per-model medians (the single headline aggregate, benchstat-style).
#[derive(Clone, Debug)]
pub struct CorpusReport {
    pub per_model: Vec<ModelStats>,
    pub geomean_of_medians: f64,
}

impl ModelStats {
    /// Summarize a model's per-seed samples.
    ///
    /// `production_seeds` is the fixed seed set used for the best-of-k proxy:
    /// `best_of_k_cost` is the min `weighted_cost` among the samples whose seed
    /// is in that set, falling back to the global min when none of the
    /// production seeds were sampled. The median seed is the sample whose cost
    /// is closest to `median_cost`, breaking ties on the lowest seed (so the
    /// chosen render is deterministic). Empty `samples` yields all-zero fields
    /// and seeds of `0` -- no panic.
    pub fn from_samples(
        model: String,
        samples: Vec<MetricSample>,
        production_seeds: &[u64],
    ) -> ModelStats {
        if samples.is_empty() {
            return ModelStats {
                model,
                samples,
                median_cost: 0.0,
                spread: (0.0, 0.0),
                best_of_k_cost: 0.0,
                best_seed: 0,
                median_seed: 0,
                worst_seed: 0,
            };
        }

        let costs: Vec<f64> = samples.iter().map(|s| s.weighted_cost).collect();
        let median_cost = median(&costs);
        let spread = (percentile(&costs, 0.25), percentile(&costs, 0.75));

        // best/worst seeds: the seeds of the global min / max weighted_cost.
        // Tie-break on the lowest seed so the chosen render is deterministic.
        let best_seed = samples
            .iter()
            .min_by(|x, y| {
                x.weighted_cost
                    .partial_cmp(&y.weighted_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(x.seed.cmp(&y.seed))
            })
            .map(|s| s.seed)
            .unwrap_or(0);
        let worst_seed = samples
            .iter()
            .max_by(|x, y| {
                x.weighted_cost
                    .partial_cmp(&y.weighted_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // For a tie on cost, max_by returns the LATER-compared-greater
                    // element; flip the seed comparison so the lowest seed wins.
                    .then(y.seed.cmp(&x.seed))
            })
            .map(|s| s.seed)
            .unwrap_or(0);

        // median seed: the sample whose cost is closest to `median_cost`,
        // breaking ties on the lowest seed.
        let median_seed = samples
            .iter()
            .min_by(|x, y| {
                let dx = (x.weighted_cost - median_cost).abs();
                let dy = (y.weighted_cost - median_cost).abs();
                dx.partial_cmp(&dy)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(x.seed.cmp(&y.seed))
            })
            .map(|s| s.seed)
            .unwrap_or(0);

        // best-of-k: min weighted_cost among samples whose seed is a production
        // seed; fall back to the global min when none were sampled.
        let prod_min = samples
            .iter()
            .filter(|s| production_seeds.contains(&s.seed))
            .map(|s| s.weighted_cost)
            .fold(f64::INFINITY, f64::min);
        let best_of_k_cost = if prod_min.is_finite() {
            prod_min
        } else {
            costs.iter().cloned().fold(f64::INFINITY, f64::min)
        };

        ModelStats {
            model,
            samples,
            median_cost,
            spread,
            best_of_k_cost,
            best_seed,
            median_seed,
            worst_seed,
        }
    }
}

impl CorpusReport {
    /// Build a corpus report. `geomean_of_medians` is the geometric mean of
    /// each model's `median_cost`, with each median floored by
    /// [`GEOMEAN_FLOOR_EPSILON`] so a single `0` median cannot zero the whole
    /// aggregate. An empty corpus yields `geomean_of_medians == 0.0`.
    pub fn from_model_stats(per_model: Vec<ModelStats>) -> CorpusReport {
        let medians: Vec<f64> = per_model
            .iter()
            .map(|m| m.median_cost.max(GEOMEAN_FLOOR_EPSILON))
            .collect();
        let geomean_of_medians = geomean(&medians);
        CorpusReport {
            per_model,
            geomean_of_medians,
        }
    }
}

/// Per-model verdict from comparing a baseline against a candidate report.
#[derive(Clone, Debug)]
pub struct ModelComparison {
    pub model: String,
    pub baseline_median: f64,
    pub candidate_median: f64,
    /// `candidate_median / baseline_median - 1.0`, or `0.0` when the baseline
    /// median is `0` (so a degenerate baseline never produces inf/NaN). A
    /// negative ratio means the candidate is cheaper (better).
    pub delta_ratio: f64,
    /// Two-sided Mann-Whitney U p-value over the two models' seed-sample
    /// `weighted_cost` vectors.
    pub p_value: f64,
    /// `p_value < SIGNIFICANCE_ALPHA`.
    pub significant: bool,
}

/// Result of comparing two corpus reports: one [`ModelComparison`] per matched
/// model plus the corpus-wide aggregate delta and significance verdict.
#[derive(Clone, Debug)]
pub struct Comparison {
    /// One entry per model present in BOTH reports (unmatched models are
    /// skipped -- see [`compare`]), in baseline iteration order.
    pub per_model: Vec<ModelComparison>,
    /// `geomean(candidate medians) / geomean(baseline medians) - 1.0` over the
    /// matched per-model medians, or `0.0` when the baseline geomean is `0`.
    pub aggregate_delta_ratio: f64,
    /// Two-sided Mann-Whitney U p-value over the matched per-model medians (see
    /// [`compare`] for why Mann-Whitney rather than a paired test).
    pub aggregate_p_value: f64,
    /// `aggregate_p_value < SIGNIFICANCE_ALPHA`.
    pub aggregate_significant: bool,
}

/// Significance threshold for the p-value verdicts -- the conventional 5%.
pub const SIGNIFICANCE_ALPHA: f64 = 0.05;

/// Compute `candidate / baseline - 1.0`, returning `0.0` when `baseline == 0`
/// so a degenerate (zero) baseline never produces an infinite or NaN ratio.
/// Mirrors the no-NaN policy of the rest of this module.
fn delta_ratio(baseline: f64, candidate: f64) -> f64 {
    if baseline == 0.0 {
        0.0
    } else {
        candidate / baseline - 1.0
    }
}

/// Compare two corpus reports.
///
/// Models are matched by `model` name; only models present in BOTH reports are
/// compared. A model present in just one report is **skipped** (it has no
/// counterpart to difference against). The returned `per_model` is in baseline
/// iteration order.
///
/// Per matched model: the two seed-sample `weighted_cost` vectors are run
/// through [`mann_whitney_u`]; `delta_ratio` is computed from the medians
/// (`0.0` when the baseline median is `0`); `significant` is
/// `p_value < SIGNIFICANCE_ALPHA`.
///
/// Aggregate: `aggregate_delta_ratio` is the ratio of the candidate-side to
/// baseline-side geometric mean of the matched per-model medians (each side
/// floored by [`GEOMEAN_FLOOR_EPSILON`] exactly as [`CorpusReport`] does, so a
/// `0` median can't zero the aggregate). `aggregate_p_value` is
/// `mann_whitney_u(baseline_medians, candidate_medians).p_value` over the
/// matched per-model medians.
///
/// The aggregate significance test treats the two median vectors as
/// independent samples (Mann-Whitney U), per the design. A paired test such as
/// Wilcoxon signed-rank -- which would exploit the model-by-model pairing of
/// the matched medians -- is a documented future refinement, not implemented
/// here.
///
/// On empty or fully-disjoint reports there are no matched models:
/// `per_model` is empty, `aggregate_delta_ratio == 0.0`, and the aggregate is
/// non-significant with a finite p-value (no NaN).
pub fn compare(baseline: &CorpusReport, candidate: &CorpusReport) -> Comparison {
    // Index the candidate's models by name so we can pull the matching entry in
    // baseline iteration order without an O(n^2) scan.
    let candidate_by_name: std::collections::HashMap<&str, &ModelStats> = candidate
        .per_model
        .iter()
        .map(|m| (m.model.as_str(), m))
        .collect();

    let mut per_model = Vec::new();
    let mut baseline_medians = Vec::new();
    let mut candidate_medians = Vec::new();

    for base in &baseline.per_model {
        let Some(cand) = candidate_by_name.get(base.model.as_str()) else {
            // Unmatched: present only in the baseline, so skip it.
            continue;
        };

        let baseline_costs: Vec<f64> = base.samples.iter().map(|s| s.weighted_cost).collect();
        let candidate_costs: Vec<f64> = cand.samples.iter().map(|s| s.weighted_cost).collect();
        let mw = mann_whitney_u(&baseline_costs, &candidate_costs);

        let baseline_median = base.median_cost;
        let candidate_median = cand.median_cost;
        let ratio = delta_ratio(baseline_median, candidate_median);

        baseline_medians.push(baseline_median);
        candidate_medians.push(candidate_median);

        per_model.push(ModelComparison {
            model: base.model.clone(),
            baseline_median,
            candidate_median,
            delta_ratio: ratio,
            p_value: mw.p_value,
            significant: mw.p_value < SIGNIFICANCE_ALPHA,
        });
    }

    // Aggregate delta: ratio of the two geomean-of-medians, each side floored
    // by the same epsilon CorpusReport uses so a single 0 median can't zero a
    // side's geometric mean.
    let baseline_floored: Vec<f64> = baseline_medians
        .iter()
        .map(|&m| m.max(GEOMEAN_FLOOR_EPSILON))
        .collect();
    let candidate_floored: Vec<f64> = candidate_medians
        .iter()
        .map(|&m| m.max(GEOMEAN_FLOOR_EPSILON))
        .collect();
    let aggregate_delta_ratio =
        delta_ratio(geomean(&baseline_floored), geomean(&candidate_floored));

    let aggregate_p_value = mann_whitney_u(&baseline_medians, &candidate_medians).p_value;

    Comparison {
        per_model,
        aggregate_delta_ratio,
        aggregate_p_value,
        aggregate_significant: aggregate_p_value < SIGNIFICANCE_ALPHA,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const EPS: f64 = 1e-9;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    // --- geomean ---

    #[test]
    fn test_geomean_two_values() {
        // sqrt(2*8) = sqrt(16) = 4.
        assert!(close(geomean(&[2.0, 8.0]), 4.0), "{}", geomean(&[2.0, 8.0]));
    }

    #[test]
    fn test_geomean_three_values() {
        // cbrt(1*10*100) = cbrt(1000) = 10.
        let g = geomean(&[1.0, 10.0, 100.0]);
        assert!(close(g, 10.0), "{}", g);
    }

    #[test]
    fn test_geomean_empty_is_zero() {
        assert_eq!(geomean(&[]), 0.0);
    }

    #[test]
    fn test_geomean_single() {
        assert_eq!(geomean(&[5.0]), 5.0);
    }

    // --- percentile / median (type 7) ---

    #[test]
    fn test_median_odd() {
        assert_eq!(median(&[1.0, 2.0, 3.0]), 2.0);
    }

    #[test]
    fn test_median_even() {
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), 2.5);
    }

    #[test]
    fn test_percentile_type7_quartiles() {
        // NumPy np.percentile([1,2,3,4,5], 25) == 2.0, 75 == 4.0.
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.25), 2.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.75), 4.0);
    }

    #[test]
    fn test_percentile_empty_is_zero() {
        assert_eq!(percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn test_percentile_single() {
        assert_eq!(percentile(&[7.0], 0.9), 7.0);
    }

    #[test]
    fn test_percentile_unsorted_input() {
        // The function must sort a copy: a reversed input gives the same answer.
        assert_eq!(percentile(&[5.0, 4.0, 3.0, 2.0, 1.0], 0.25), 2.0);
    }

    #[test]
    fn test_percentile_endpoints() {
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.0), 1.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 1.0), 5.0);
    }

    // --- Mann-Whitney U ---

    #[test]
    fn test_mann_whitney_complete_separation() {
        // a strictly below b: complete separation. With n1 = n2 = 4,
        // r1 = 1+2+3+4 = 10, u1 = 10 - 4*5/2 = 0, u2 = 16 - 0 = 16, u = 0.
        let r = mann_whitney_u(&[1.0, 2.0, 3.0, 4.0], &[5.0, 6.0, 7.0, 8.0]);
        assert_eq!(r.u1, 0.0);
        assert_eq!(r.u2, 16.0);
        assert_eq!(r.u, 0.0);
        assert!(
            r.p_value < 0.05,
            "p_value {} should be significant",
            r.p_value
        );
    }

    #[test]
    fn test_mann_whitney_no_difference() {
        // Identical samples: every value tied. u1 == u2 == n1*n2/2 == 8, and
        // the tie-corrected variance is 0, so p_value is the non-significant
        // default of 1.0.
        let r = mann_whitney_u(&[1.0, 2.0, 3.0, 4.0], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(r.u1, 8.0);
        assert_eq!(r.u2, 8.0);
        assert!(
            r.p_value > 0.5,
            "p_value {} should be non-significant",
            r.p_value
        );
    }

    #[test]
    fn test_mann_whitney_u1_plus_u2_invariant() {
        // u1 + u2 == n1*n2 on a mixed (interleaved, with ties) example.
        let a = [1.0, 3.0, 5.0, 7.0, 3.0];
        let b = [2.0, 4.0, 6.0, 3.0];
        let r = mann_whitney_u(&a, &b);
        let n1n2 = (a.len() * b.len()) as f64;
        assert!(
            close(r.u1 + r.u2, n1n2),
            "u1 {} + u2 {} != n1*n2 {}",
            r.u1,
            r.u2,
            n1n2
        );
    }

    #[test]
    fn test_mann_whitney_empty_is_nonsignificant() {
        let r = mann_whitney_u(&[], &[1.0, 2.0, 3.0]);
        assert_eq!(r.p_value, 1.0);
        assert!(r.u.is_finite());
        assert!(r.u1.is_finite());
        assert!(r.u2.is_finite());
    }

    // --- erf / Phi sanity (exercised indirectly through the p-value path) ---

    #[test]
    fn test_phi_zero() {
        assert!(close(phi(0.0), 0.5), "{}", phi(0.0));
    }

    #[test]
    fn test_phi_1_96() {
        // The classic 97.5th percentile of the standard normal.
        assert!((phi(1.96) - 0.975).abs() < 1e-3, "{}", phi(1.96));
    }

    #[test]
    fn test_erf_known_values() {
        assert!(close(erf(0.0), 0.0), "{}", erf(0.0));
        // erf(1) ~= 0.8427007929 (A&S 7.1.26 max error ~1.5e-7).
        assert!((erf(1.0) - 0.842_700_792_9).abs() < 1e-6, "{}", erf(1.0));
        // erf is odd.
        assert!(close(erf(-0.5), -erf(0.5)), "erf not odd");
    }

    // --- No NaN: every primitive on empty / degenerate input is finite ---

    #[test]
    fn test_no_nan_on_degenerate_input() {
        assert!(geomean(&[]).is_finite());
        assert!(geomean(&[3.0]).is_finite());
        assert!(percentile(&[], 0.5).is_finite());
        assert!(percentile(&[1.0], 0.5).is_finite());
        assert!(median(&[]).is_finite());
        let r0 = mann_whitney_u(&[], &[]);
        assert!(r0.u.is_finite() && r0.u1.is_finite() && r0.u2.is_finite());
        assert!(r0.p_value.is_finite());
        let r1 = mann_whitney_u(&[1.0, 1.0], &[1.0, 1.0]);
        assert!(r1.p_value.is_finite());
        assert!(phi(0.0).is_finite());
        assert!(erf(0.0).is_finite());
    }

    // --- property tests for the statistics invariants ---

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// The geometric mean is a function of the multiset of values: it is
        /// invariant under any permutation of the input (the product of the
        /// values is commutative).
        #[test]
        fn prop_geomean_permutation_invariant(
            mut vals in prop::collection::vec(0.01f64..1000.0, 1..=12),
            seed in any::<u64>(),
        ) {
            let base = geomean(&vals);
            // Deterministic Fisher-Yates shuffle driven by `seed` so the
            // property is a pure rearrangement of the same multiset.
            let mut state = seed | 1;
            for i in (1..vals.len()).rev() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let j = (state >> 33) as usize % (i + 1);
                vals.swap(i, j);
            }
            let shuffled = geomean(&vals);
            // Relative tolerance: ln/exp accumulates rounding across orderings.
            prop_assert!(
                (base - shuffled).abs() <= 1e-9 * base.abs().max(1.0),
                "geomean changed under permutation: {} vs {}",
                base,
                shuffled
            );
        }

        /// `percentile` is bounded by the sample's min and max and is monotone
        /// non-decreasing in `p`. Both are core type-7 invariants and both must
        /// produce finite values.
        #[test]
        fn prop_percentile_bounded_and_monotone(
            vals in prop::collection::vec(-500.0f64..500.0, 1..=20),
            p_lo in 0.0f64..=1.0,
            delta in 0.0f64..=1.0,
        ) {
            let p_hi = (p_lo + delta).min(1.0);
            let min = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let max = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let q_lo = percentile(&vals, p_lo);
            let q_hi = percentile(&vals, p_hi);
            prop_assert!(q_lo.is_finite() && q_hi.is_finite());
            // Bounded by the data range (small slack for interpolation rounding).
            prop_assert!(q_lo >= min - 1e-9 && q_lo <= max + 1e-9, "{} not in [{},{}]", q_lo, min, max);
            // Monotone non-decreasing in p.
            prop_assert!(q_hi >= q_lo - 1e-9, "percentile not monotone: {} < {}", q_hi, q_lo);
        }

        /// The partition identity `u1 + u2 == n1 * n2` holds for ANY pair of
        /// non-empty samples, and the reported `u` is the smaller of the two.
        /// The two-sided p-value is always a finite probability in [0, 1].
        #[test]
        fn prop_mann_whitney_partition_identity(
            a in prop::collection::vec(-50.0f64..50.0, 1..=15),
            b in prop::collection::vec(-50.0f64..50.0, 1..=15),
        ) {
            let r = mann_whitney_u(&a, &b);
            let n1n2 = (a.len() * b.len()) as f64;
            prop_assert!(
                (r.u1 + r.u2 - n1n2).abs() < 1e-9,
                "u1 {} + u2 {} != n1*n2 {}",
                r.u1, r.u2, n1n2
            );
            prop_assert!((r.u - r.u1.min(r.u2)).abs() < 1e-9);
            prop_assert!(r.p_value.is_finite() && (0.0..=1.0).contains(&r.p_value));
        }
    }

    // --- Task 2: ModelStats / CorpusReport constructors ---

    /// A `LayoutMetrics` whose `node_overlap` carries `cost` and every other
    /// term is zero, so `weighted_cost` with `node_overlap == 1.0` returns
    /// exactly `cost`. Keeps the test fixtures readable while still exercising
    /// the real struct.
    fn metrics_with_cost(cost: f64) -> LayoutMetrics {
        LayoutMetrics {
            node_overlap: cost,
            node_connector_overlap: 0.0,
            label_overlap: 0.0,
            crossings: 0.0,
            sprawl: 0.0,
            edge_length_cv: 0.0,
            aspect_penalty: 0.0,
            chain_straightness: 0.0,
            loop_compactness: 0.0,
        }
    }

    fn sample(seed: u64, cost: f64) -> MetricSample {
        MetricSample {
            seed,
            metrics: metrics_with_cost(cost),
            weighted_cost: cost,
        }
    }

    #[test]
    fn test_from_samples_known_set() {
        // Five seeds with hand-pickable costs.
        //   seed 1 -> 10, seed 2 -> 30, seed 3 -> 20, seed 4 -> 50, seed 5 -> 40
        // Sorted costs: [10, 20, 30, 40, 50].
        //   median (type-7, p=0.5) = 30
        //   p25 = 20, p75 = 40
        //   global min cost = 10 (seed 1), max cost = 50 (seed 4)
        //   median-nearest cost = 30 (seed 2)
        let samples = vec![
            sample(1, 10.0),
            sample(2, 30.0),
            sample(3, 20.0),
            sample(4, 50.0),
            sample(5, 40.0),
        ];
        // Production seeds: 3 and 5 (costs 20 and 40). Min over them is 20, which
        // is NOT the global min (10, seed 1). This is the "best-of-k differs from
        // the global min" case.
        let production_seeds = [3u64, 5u64];
        let stats = ModelStats::from_samples("m".to_string(), samples, &production_seeds);

        assert_eq!(stats.model, "m");
        assert_eq!(stats.median_cost, 30.0);
        assert_eq!(stats.spread, (20.0, 40.0));
        assert_eq!(
            stats.best_of_k_cost, 20.0,
            "best-of-k must use production seeds"
        );
        assert_eq!(stats.best_seed, 1, "global min cost is seed 1");
        assert_eq!(stats.worst_seed, 4, "global max cost is seed 4");
        assert_eq!(stats.median_seed, 2, "median-nearest cost is seed 2");
    }

    #[test]
    fn test_from_samples_best_of_k_falls_back_to_global_min() {
        // No production seed was sampled -> best_of_k_cost falls back to global
        // min weighted_cost.
        let samples = vec![sample(1, 10.0), sample(2, 30.0), sample(3, 20.0)];
        let production_seeds = [100u64, 200u64];
        let stats = ModelStats::from_samples("m".to_string(), samples, &production_seeds);
        assert_eq!(
            stats.best_of_k_cost, 10.0,
            "no production seed sampled -> global min"
        );
    }

    #[test]
    fn test_from_samples_median_seed_tie_break_lowest() {
        // Two seeds equidistant from the median cost: the lower seed wins.
        //   seeds 5, 9 with costs 10 and 30; sorted costs [10, 30] -> median 20.
        //   |10 - 20| == |30 - 20| == 10, a tie. Lowest seed (5) must win.
        let samples = vec![sample(9, 30.0), sample(5, 10.0)];
        let stats = ModelStats::from_samples("m".to_string(), samples, &[]);
        assert_eq!(stats.median_cost, 20.0);
        assert_eq!(stats.median_seed, 5, "tie must break on the lowest seed");
    }

    #[test]
    fn test_from_samples_worst_seed_tie_break_lowest() {
        // Two seeds SHARE the maximum cost; the lower seed must win. The third
        // (lower-cost) sample ensures the max is a genuine tie, not the only
        // value. seeds 7 and 4 both cost 50 (the max); seed 2 costs 10.
        // worst_seed must be 4 (the lower of the two tied-at-max seeds), NOT 7.
        // This fails if the tie-break direction in from_samples were reversed
        // (a `.then(x.seed.cmp(&y.seed))` after max_by would pick 7).
        let samples = vec![sample(7, 50.0), sample(2, 10.0), sample(4, 50.0)];
        let stats = ModelStats::from_samples("m".to_string(), samples, &[]);
        assert_eq!(
            stats.worst_seed, 4,
            "max-cost tie must break on the lowest seed"
        );
    }

    #[test]
    fn test_from_samples_empty_is_all_zero() {
        let stats = ModelStats::from_samples("empty".to_string(), vec![], &[1, 2, 3]);
        assert_eq!(stats.median_cost, 0.0);
        assert_eq!(stats.spread, (0.0, 0.0));
        assert_eq!(stats.best_of_k_cost, 0.0);
        assert_eq!(stats.best_seed, 0);
        assert_eq!(stats.median_seed, 0);
        assert_eq!(stats.worst_seed, 0);
        // Finite, no NaN.
        assert!(stats.median_cost.is_finite());
        assert!(stats.spread.0.is_finite() && stats.spread.1.is_finite());
        assert!(stats.best_of_k_cost.is_finite());
    }

    fn model_stats_with_median(model: &str, median: f64) -> ModelStats {
        // Build a one-sample model whose median equals `median`.
        ModelStats::from_samples(model.to_string(), vec![sample(1, median)], &[1])
    }

    #[test]
    fn test_from_model_stats_geomean_of_medians() {
        // Three models with medians 2, 8, 32: geomean = cbrt(2*8*32) = cbrt(512) = 8.
        let per_model = vec![
            model_stats_with_median("a", 2.0),
            model_stats_with_median("b", 8.0),
            model_stats_with_median("c", 32.0),
        ];
        let medians: Vec<f64> = per_model.iter().map(|m| m.median_cost).collect();
        let report = CorpusReport::from_model_stats(per_model);
        assert!(
            close(report.geomean_of_medians, geomean(&medians)),
            "{} != {}",
            report.geomean_of_medians,
            geomean(&medians)
        );
        assert!(
            close(report.geomean_of_medians, 8.0),
            "{}",
            report.geomean_of_medians
        );
    }

    #[test]
    fn test_from_model_stats_zero_median_does_not_zero_aggregate() {
        // A model with median 0 must not collapse the corpus geomean to 0; the
        // epsilon floor keeps it positive and finite.
        let per_model = vec![
            model_stats_with_median("a", 0.0),
            model_stats_with_median("b", 10.0),
            model_stats_with_median("c", 1000.0),
        ];
        let report = CorpusReport::from_model_stats(per_model);
        assert!(
            report.geomean_of_medians > 0.0,
            "a single 0 median must not zero the aggregate: got {}",
            report.geomean_of_medians
        );
        assert!(report.geomean_of_medians.is_finite());
        // It must equal the geomean of the floored medians, exactly.
        let floored = [GEOMEAN_FLOOR_EPSILON, 10.0, 1000.0];
        assert!(
            close(report.geomean_of_medians, geomean(&floored)),
            "{} != {}",
            report.geomean_of_medians,
            geomean(&floored)
        );
    }

    #[test]
    fn test_from_model_stats_empty_corpus_is_zero() {
        let report = CorpusReport::from_model_stats(vec![]);
        assert_eq!(report.geomean_of_medians, 0.0);
        assert!(report.geomean_of_medians.is_finite());
    }

    // --- Task 3: compare(baseline, candidate) ---

    /// Build a `ModelStats` directly from a list of `(seed, cost)` pairs, with
    /// no production seeds (best-of-k irrelevant for the comparison tests).
    fn model_stats_from_costs(model: &str, seed_costs: &[(u64, f64)]) -> ModelStats {
        let samples: Vec<MetricSample> = seed_costs
            .iter()
            .map(|&(seed, cost)| sample(seed, cost))
            .collect();
        ModelStats::from_samples(model.to_string(), samples, &[])
    }

    #[test]
    fn test_compare_identical_report_is_zero_and_nonsignificant() {
        // AC4.5: comparing a report against itself must report no change and no
        // significance, with p-values pinned to the non-significant default.
        let report = CorpusReport::from_model_stats(vec![
            model_stats_from_costs("a", &[(1, 10.0), (2, 20.0), (3, 30.0), (4, 40.0)]),
            model_stats_from_costs("b", &[(1, 5.0), (2, 15.0), (3, 25.0), (4, 35.0)]),
        ]);

        let cmp = compare(&report, &report);

        assert_eq!(cmp.per_model.len(), 2);
        for m in &cmp.per_model {
            assert_eq!(m.delta_ratio, 0.0, "model {} delta_ratio", m.model);
            assert!(!m.significant, "model {} must not be significant", m.model);
            // Identical seed samples ⇒ every value tied ⇒ non-significant.
            assert!(
                m.p_value > 0.5,
                "model {} p_value {} should be non-significant",
                m.model,
                m.p_value
            );
        }
        assert_eq!(cmp.aggregate_delta_ratio, 0.0);
        assert!(!cmp.aggregate_significant);
        assert!(
            cmp.aggregate_p_value > 0.5,
            "aggregate p_value {} should be non-significant",
            cmp.aggregate_p_value
        );
    }

    #[test]
    fn test_compare_clear_improvement_is_negative_and_significant() {
        // Candidate strictly below baseline with non-overlapping seed samples:
        // the aggregate delta is negative and the per-model verdict is
        // significant where the two samples completely separate.
        let baseline = CorpusReport::from_model_stats(vec![
            model_stats_from_costs(
                "a",
                &[(1, 100.0), (2, 110.0), (3, 120.0), (4, 130.0), (5, 140.0)],
            ),
            model_stats_from_costs(
                "b",
                &[(1, 200.0), (2, 210.0), (3, 220.0), (4, 230.0), (5, 240.0)],
            ),
        ]);
        let candidate = CorpusReport::from_model_stats(vec![
            model_stats_from_costs(
                "a",
                &[(1, 10.0), (2, 11.0), (3, 12.0), (4, 13.0), (5, 14.0)],
            ),
            model_stats_from_costs(
                "b",
                &[(1, 20.0), (2, 21.0), (3, 22.0), (4, 23.0), (5, 24.0)],
            ),
        ]);

        let cmp = compare(&baseline, &candidate);

        assert_eq!(cmp.per_model.len(), 2);
        for m in &cmp.per_model {
            assert!(
                m.delta_ratio < 0.0,
                "model {} delta_ratio {} should be negative",
                m.model,
                m.delta_ratio
            );
            assert!(
                m.candidate_median < m.baseline_median,
                "model {} candidate median {} should be below baseline {}",
                m.model,
                m.candidate_median,
                m.baseline_median
            );
            assert!(
                m.significant,
                "model {} (completely separated samples) should be significant; p_value {}",
                m.model, m.p_value
            );
        }
        assert!(
            cmp.aggregate_delta_ratio < 0.0,
            "aggregate_delta_ratio {} should be negative",
            cmp.aggregate_delta_ratio
        );
    }

    #[test]
    fn test_compare_only_matched_models_are_compared() {
        // Models are matched by name; a model present in only one report is
        // skipped. baseline has {a, b, only_baseline}; candidate has
        // {a, b, only_candidate}. The matched set compared is {a, b}, in
        // baseline order.
        let baseline = CorpusReport::from_model_stats(vec![
            model_stats_from_costs("only_baseline", &[(1, 1.0), (2, 2.0)]),
            model_stats_from_costs("a", &[(1, 10.0), (2, 20.0), (3, 30.0)]),
            model_stats_from_costs("b", &[(1, 100.0), (2, 200.0), (3, 300.0)]),
        ]);
        let candidate = CorpusReport::from_model_stats(vec![
            model_stats_from_costs("b", &[(1, 100.0), (2, 200.0), (3, 300.0)]),
            model_stats_from_costs("a", &[(1, 10.0), (2, 20.0), (3, 30.0)]),
            model_stats_from_costs("only_candidate", &[(1, 9.0), (2, 8.0)]),
        ]);

        let cmp = compare(&baseline, &candidate);

        // Exactly the two matched models, in baseline iteration order.
        let names: Vec<&str> = cmp.per_model.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(
            names,
            vec!["a", "b"],
            "only matched models, in baseline order"
        );
        // The unmatched names appear nowhere.
        assert!(!names.contains(&"only_baseline"));
        assert!(!names.contains(&"only_candidate"));
    }

    #[test]
    fn test_compare_zero_baseline_median_no_divide_by_zero() {
        // No NaN: a model whose baseline median is 0 yields delta_ratio == 0.0
        // (not inf/NaN) and every reported field stays finite.
        let baseline = CorpusReport::from_model_stats(vec![model_stats_from_costs(
            "z",
            &[(1, 0.0), (2, 0.0), (3, 0.0)],
        )]);
        let candidate = CorpusReport::from_model_stats(vec![model_stats_from_costs(
            "z",
            &[(1, 5.0), (2, 6.0), (3, 7.0)],
        )]);

        let cmp = compare(&baseline, &candidate);

        assert_eq!(cmp.per_model.len(), 1);
        let m = &cmp.per_model[0];
        assert_eq!(m.baseline_median, 0.0);
        assert_eq!(
            m.delta_ratio, 0.0,
            "delta_ratio with a 0 baseline median must be 0.0, not inf/NaN"
        );
        assert!(m.delta_ratio.is_finite());
        assert!(m.candidate_median.is_finite());
        assert!(m.p_value.is_finite());
        assert!(cmp.aggregate_delta_ratio.is_finite());
        assert!(cmp.aggregate_p_value.is_finite());
    }

    #[test]
    fn test_compare_empty_reports_are_finite_and_nonsignificant() {
        // Degenerate input: two empty corpora compare to no per-model rows, a
        // zero aggregate delta, and a finite non-significant verdict.
        let empty = CorpusReport::from_model_stats(vec![]);
        let cmp = compare(&empty, &empty);
        assert!(cmp.per_model.is_empty());
        assert_eq!(cmp.aggregate_delta_ratio, 0.0);
        assert!(cmp.aggregate_delta_ratio.is_finite());
        assert!(cmp.aggregate_p_value.is_finite());
        assert!(!cmp.aggregate_significant);
    }

    #[test]
    fn test_compare_no_matched_models_is_finite() {
        // Reports with disjoint model names share no matched models: no
        // per-model rows, a zero aggregate delta, and a finite verdict.
        let baseline =
            CorpusReport::from_model_stats(vec![model_stats_from_costs("a", &[(1, 10.0)])]);
        let candidate =
            CorpusReport::from_model_stats(vec![model_stats_from_costs("b", &[(1, 20.0)])]);
        let cmp = compare(&baseline, &candidate);
        assert!(cmp.per_model.is_empty());
        assert_eq!(cmp.aggregate_delta_ratio, 0.0);
        assert!(cmp.aggregate_delta_ratio.is_finite());
        assert!(cmp.aggregate_p_value.is_finite());
        assert!(!cmp.aggregate_significant);
    }

    #[test]
    fn test_compare_significance_alpha_is_five_percent() {
        // The exported significance threshold is the conventional 0.05.
        assert_eq!(SIGNIFICANCE_ALPHA, 0.05);
    }
}
