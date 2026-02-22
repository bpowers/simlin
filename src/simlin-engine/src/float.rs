// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Floating-point utility functions for the simulation engine.

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
}
