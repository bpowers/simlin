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
}
