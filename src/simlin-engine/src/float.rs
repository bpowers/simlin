// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Floating-point utility functions for the simulation engine.

/// Vensim's `:NA:` ("missing data") sentinel: the *finite* number `-2^109`.
///
/// Vensim's `:NA:` is NOT IEEE NaN -- it is an ordinary finite value used to
/// "test for the existence of data" via the idiom `IF THEN ELSE(x = :NA:, ...)`.
/// That existence test only works because `:NA:` is finite: ordinary `=`
/// equality (`approx_eq`) matches the sentinel against itself, and arithmetic on
/// `:NA:` computes a finite result rather than poisoning the expression the way
/// NaN (which is absorbing) would. Both `:NA:` paths in the engine -- the
/// expression literal (via the MDL->XMILE formatter) and the data-list literal
/// (via the MDL number-list parser) -- route to this single constant so the
/// representation is consistent and Vensim-faithful.
///
/// `-2^109` is exactly representable in f64 (its mantissa is zero), so the
/// literal below is bit-identical to `-(2.0_f64).powi(109)` (pinned by
/// `na_is_negative_two_pow_109`). At this magnitude the exponent field alone
/// distinguishes it from neighbours like `-2^110`, so `approx_eq` never
/// spuriously equates the sentinel with a contaminated value.
pub const NA: f64 = -6.490371073168535e32;

/// ULP-based approximate equality for f64, matching the semantics of the
/// `float_cmp::approx_eq!` macro used throughout the codebase.
#[inline(always)]
pub fn approx_eq(a: f64, b: f64) -> bool {
    float_cmp::approx_eq!(f64, a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_approx_eq_matches_float_cmp() {
        let a: f64 = 1.0;
        let b: f64 = 1.0 + f64::EPSILON;
        assert!(approx_eq(a, b));
    }

    #[test]
    fn f64_not_approx_eq() {
        assert!(!approx_eq(1.0, 2.0));
    }

    #[test]
    fn f64_approx_eq_nan() {
        // float_cmp::approx_eq! treats NaN == NaN (ULP-based comparison)
        assert!(approx_eq(f64::NAN, f64::NAN));
    }

    #[test]
    fn na_is_negative_two_pow_109() {
        // Pin the canonical Vensim :NA: sentinel to the exact f64 value -2^109.
        // -2^109 is exactly representable (zero mantissa), so the spelled-out
        // literal must be bit-identical to the computed power of two.
        assert_eq!(NA.to_bits(), (-(2.0_f64).powi(109)).to_bits());
        assert_eq!(NA, -(2.0_f64).powi(109));
        assert!(NA.is_finite(), ":NA: sentinel must be finite, never NaN");
    }

    #[test]
    fn na_existence_test_via_approx_eq() {
        // The Vensim existence test `x = :NA:` is ordinary `=` equality against
        // the sentinel. approx_eq must match :NA: against itself (test fires)...
        assert!(approx_eq(NA, NA), ":NA: must equal itself (existence test)");
        // ...and must NOT match genuine values, including a doubled/contaminated
        // magnitude (-2^110): at this exponent the gap is ~2^52 ULPs, far beyond
        // float_cmp's tolerance, so no spurious existence-test hit occurs.
        assert!(!approx_eq(NA, 0.0));
        assert!(!approx_eq(NA, -(2.0_f64).powi(110)));
        // :NA: arithmetic stays finite (NaN would poison it).
        assert!((NA + 10.0).is_finite());
    }
}
