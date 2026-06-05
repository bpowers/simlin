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

/// ULP-based approximate equality for f64.
///
/// Two values are equal when any of these hold, checked in order:
/// 1. exact equality (the common case, one compare);
/// 2. absolute difference <= `f64::EPSILON` (covers comparisons near zero,
///    where the ULP distance between tiny values of opposite sign explodes);
/// 3. at most 4 ULPs (units of least precision) apart.
///
/// These are the default-margin semantics of the `float_cmp` crate this
/// function replaced (`epsilon = f64::EPSILON, ulps = 4`), preserved exactly
/// so existing comparison behavior (including the `:NA:` existence test
/// below) is unchanged. Note two deliberate ULP-arm consequences: identical
/// NaN bit patterns compare equal (0 ULPs apart), and `f64::MAX` equals
/// `+inf` (1 ULP apart).
#[inline(always)]
pub fn approx_eq(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    if (a - b).abs() <= f64::EPSILON {
        return true;
    }
    let ulps = ordered_bits(a).wrapping_sub(ordered_bits(b));
    ulps.saturating_abs() <= 4
}

/// [`approx_eq`] with a caller-chosen absolute-difference tolerance in place
/// of `f64::EPSILON`; the 4-ULP arm is unchanged. Matches `float_cmp`'s
/// `approx_eq!(f64, a, b, epsilon = e)` (which overrode only the epsilon half
/// of the default margin). Used by tests comparing simulation results across
/// backends, where tolerances are looser than machine epsilon.
#[inline(always)]
pub fn approx_eq_eps(a: f64, b: f64, epsilon: f64) -> bool {
    if a == b {
        return true;
    }
    if (a - b).abs() <= epsilon {
        return true;
    }
    let ulps = ordered_bits(a).wrapping_sub(ordered_bits(b));
    ulps.saturating_abs() <= 4
}

/// Map an f64's sign-magnitude bit pattern onto a single continuous integer
/// line so consecutive representable floats differ by exactly 1, including
/// across the +/-0 boundary: negative floats (descending bit patterns) are
/// bit-inverted, positive floats are shifted above them by flipping the sign
/// bit.
#[inline(always)]
fn ordered_bits(f: f64) -> i64 {
    const SIGN_BIT: u64 = 1 << 63;
    let bits = f.to_bits();
    (if bits & SIGN_BIT != 0 {
        !bits
    } else {
        bits ^ SIGN_BIT
    }) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_approx_eq_adjacent() {
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
        // identical NaN bit patterns are 0 ULPs apart, so they compare equal
        assert!(approx_eq(f64::NAN, f64::NAN));
    }

    #[test]
    fn f64_approx_eq_epsilon_near_zero() {
        // Near zero the ULP distance between tiny values of opposite sign is
        // enormous, so equality there must come from the absolute-epsilon arm.
        assert!(approx_eq(0.0, -0.0));
        assert!(approx_eq(1e-300, -1e-300));
        assert!(approx_eq(0.0, f64::EPSILON));
        assert!(!approx_eq(0.0, 2.0 * f64::EPSILON + f64::EPSILON / 2.0));
    }

    #[test]
    fn f64_approx_eq_ulps_at_magnitude() {
        // At large magnitude the absolute difference is far above EPSILON, so
        // equality must come from the 4-ULP arm.
        let a: f64 = 1e15;
        let mut b = a;
        for _ in 0..4 {
            b = b.next_up();
        }
        assert!(approx_eq(a, b), "4 ULPs apart must compare equal");
        b = b.next_up();
        assert!(!approx_eq(a, b), "5 ULPs apart must compare unequal");
    }

    #[test]
    fn f64_approx_eq_infinities() {
        assert!(approx_eq(f64::INFINITY, f64::INFINITY));
        assert!(approx_eq(f64::NEG_INFINITY, f64::NEG_INFINITY));
        assert!(!approx_eq(f64::INFINITY, f64::NEG_INFINITY));
        // f64::MAX is the bit pattern immediately below +inf, i.e. 1 ULP away,
        // so a ULP-based comparison genuinely treats them as equal.
        assert!(approx_eq(f64::INFINITY, f64::MAX));
    }

    #[test]
    fn f64_approx_eq_sign_straddle() {
        // Values straddling zero that are more than EPSILON apart must not
        // compare equal even though their raw bit patterns are "close" when
        // naively reinterpreted; the ordered-bits mapping keeps the ULP
        // distance huge across the sign boundary.
        assert!(!approx_eq(1.0, -1.0));
        assert!(!approx_eq(1e-7, -1e-7));
    }

    #[test]
    fn f64_approx_eq_eps_custom_tolerance() {
        assert!(approx_eq_eps(1.0, 1.0 + 1e-7, 1e-6));
        assert!(!approx_eq_eps(1.0, 1.0 + 1e-5, 1e-6));
        // exact equality and the 4-ULP arm hold regardless of epsilon
        assert!(approx_eq_eps(3.5, 3.5, 0.0));
        assert!(approx_eq_eps(1e15, (1e15_f64).next_up(), 0.0));
        // NaN never within an absolute tolerance of a real value
        assert!(!approx_eq_eps(f64::NAN, 1.0, 1.0));
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
        // the 4-ULP tolerance, so no spurious existence-test hit occurs.
        assert!(!approx_eq(NA, 0.0));
        assert!(!approx_eq(NA, -(2.0_f64).powi(110)));
        // :NA: arithmetic stays finite (NaN would poison it).
        assert!((NA + 10.0).is_finite());
    }
}
