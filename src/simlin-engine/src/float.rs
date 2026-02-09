// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Trait abstracting over `f64` and `f32` for the simulation engine.
//!
//! The engine is generic over `SimFloat` so that it can run simulations in
//! either double or single precision.  The `f64` path is the production default
//! and must not regress in performance.  The `f32` path exists only for
//! validation against golden outputs produced by legacy software that used
//! single-precision arithmetic.

use std::fmt;
use std::iter::Sum;
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Rem, Sub, SubAssign};

use ordered_float::OrderedFloat;

/// A floating-point type suitable for use in the simulation engine.
///
/// This is intentionally minimal — it provides only the operations that the VM,
/// interpreter, and supporting infrastructure actually use.  Implementing for
/// `f32` and `f64` is straightforward since the methods map 1:1 to inherent
/// methods on the primitive types.
pub trait SimFloat:
    Copy
    + Clone
    + PartialEq
    + PartialOrd
    + Default
    + fmt::Debug
    + fmt::Display
    + Send
    + Sync
    + 'static
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Rem<Output = Self>
    + Neg<Output = Self>
    + AddAssign
    + SubAssign
    + MulAssign
    + DivAssign
    + Sum
{
    // ── Constants ────────────────────────────────────────────────────────

    fn zero() -> Self;
    fn one() -> Self;
    fn neg_one() -> Self;
    fn nan() -> Self;
    fn infinity() -> Self;
    fn neg_infinity() -> Self;
    fn epsilon() -> Self;
    fn pi() -> Self;
    fn half() -> Self;

    // ── Conversions ─────────────────────────────────────────────────────

    /// Convert from `f64`.  For `f32` this is a narrowing cast (`as f32`).
    fn from_f64(v: f64) -> Self;

    /// Convert to `f64`.  For `f32` this is a widening cast (`as f64`).
    fn to_f64(self) -> f64;

    /// Convert from `usize` (for array sizes, counts, etc.).
    fn from_usize(v: usize) -> Self;

    /// Convert from `i8` (for bool-to-float casts: `(cond) as i8 as F`).
    fn from_i8(v: i8) -> Self;

    // ── Classification ──────────────────────────────────────────────────

    fn is_nan(self) -> bool;

    // ── Rounding / truncation ───────────────────────────────────────────

    fn floor(self) -> Self;
    fn round(self) -> Self;
    fn trunc(self) -> Self;

    // ── Math functions ──────────────────────────────────────────────────

    fn abs(self) -> Self;
    fn sqrt(self) -> Self;
    fn powf(self, exp: Self) -> Self;
    fn exp(self) -> Self;
    fn ln(self) -> Self;
    fn log10(self) -> Self;
    fn sin(self) -> Self;
    fn cos(self) -> Self;
    fn tan(self) -> Self;
    fn asin(self) -> Self;
    fn acos(self) -> Self;
    fn atan(self) -> Self;
    fn rem_euclid(self, rhs: Self) -> Self;

    // ── Approximate equality ────────────────────────────────────────────

    /// ULP-based approximate equality, matching the semantics of the
    /// `float_cmp::approx_eq!` macro used throughout the codebase.
    fn approx_eq(self, other: Self) -> bool;

    // ── OrderedFloat interop ────────────────────────────────────────────

    /// Wrap in `OrderedFloat` for use as HashMap keys (literal dedup).
    fn to_ordered(self) -> OrderedFloat<Self>
    where
        OrderedFloat<Self>: Eq + std::hash::Hash;
}

// ════════════════════════════════════════════════════════════════════════
// f64 implementation
// ════════════════════════════════════════════════════════════════════════

impl SimFloat for f64 {
    #[inline(always)]
    fn zero() -> Self {
        0.0
    }
    #[inline(always)]
    fn one() -> Self {
        1.0
    }
    #[inline(always)]
    fn neg_one() -> Self {
        -1.0
    }
    #[inline(always)]
    fn nan() -> Self {
        f64::NAN
    }
    #[inline(always)]
    fn infinity() -> Self {
        f64::INFINITY
    }
    #[inline(always)]
    fn neg_infinity() -> Self {
        f64::NEG_INFINITY
    }
    #[inline(always)]
    fn epsilon() -> Self {
        f64::EPSILON
    }
    #[inline(always)]
    fn pi() -> Self {
        std::f64::consts::PI
    }
    #[inline(always)]
    fn half() -> Self {
        0.5
    }

    #[inline(always)]
    fn from_f64(v: f64) -> Self {
        v
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self
    }
    #[inline(always)]
    fn from_usize(v: usize) -> Self {
        v as f64
    }
    #[inline(always)]
    fn from_i8(v: i8) -> Self {
        v as f64
    }

    #[inline(always)]
    fn is_nan(self) -> bool {
        f64::is_nan(self)
    }

    #[inline(always)]
    fn floor(self) -> Self {
        f64::floor(self)
    }
    #[inline(always)]
    fn round(self) -> Self {
        f64::round(self)
    }
    #[inline(always)]
    fn trunc(self) -> Self {
        f64::trunc(self)
    }

    #[inline(always)]
    fn abs(self) -> Self {
        f64::abs(self)
    }
    #[inline(always)]
    fn sqrt(self) -> Self {
        f64::sqrt(self)
    }
    #[inline(always)]
    fn powf(self, exp: Self) -> Self {
        f64::powf(self, exp)
    }
    #[inline(always)]
    fn exp(self) -> Self {
        f64::exp(self)
    }
    #[inline(always)]
    fn ln(self) -> Self {
        f64::ln(self)
    }
    #[inline(always)]
    fn log10(self) -> Self {
        f64::log10(self)
    }
    #[inline(always)]
    fn sin(self) -> Self {
        f64::sin(self)
    }
    #[inline(always)]
    fn cos(self) -> Self {
        f64::cos(self)
    }
    #[inline(always)]
    fn tan(self) -> Self {
        f64::tan(self)
    }
    #[inline(always)]
    fn asin(self) -> Self {
        f64::asin(self)
    }
    #[inline(always)]
    fn acos(self) -> Self {
        f64::acos(self)
    }
    #[inline(always)]
    fn atan(self) -> Self {
        f64::atan(self)
    }
    #[inline(always)]
    fn rem_euclid(self, rhs: Self) -> Self {
        f64::rem_euclid(self, rhs)
    }

    #[inline(always)]
    fn approx_eq(self, other: Self) -> bool {
        float_cmp::approx_eq!(f64, self, other)
    }

    #[inline(always)]
    fn to_ordered(self) -> OrderedFloat<Self> {
        OrderedFloat(self)
    }
}

// ════════════════════════════════════════════════════════════════════════
// f32 implementation
// ════════════════════════════════════════════════════════════════════════

impl SimFloat for f32 {
    #[inline(always)]
    fn zero() -> Self {
        0.0
    }
    #[inline(always)]
    fn one() -> Self {
        1.0
    }
    #[inline(always)]
    fn neg_one() -> Self {
        -1.0
    }
    #[inline(always)]
    fn nan() -> Self {
        f32::NAN
    }
    #[inline(always)]
    fn infinity() -> Self {
        f32::INFINITY
    }
    #[inline(always)]
    fn neg_infinity() -> Self {
        f32::NEG_INFINITY
    }
    #[inline(always)]
    fn epsilon() -> Self {
        f32::EPSILON
    }
    #[inline(always)]
    fn pi() -> Self {
        std::f32::consts::PI
    }
    #[inline(always)]
    fn half() -> Self {
        0.5
    }

    #[inline(always)]
    fn from_f64(v: f64) -> Self {
        v as f32
    }
    #[inline(always)]
    fn to_f64(self) -> f64 {
        self as f64
    }
    #[inline(always)]
    fn from_usize(v: usize) -> Self {
        v as f32
    }
    #[inline(always)]
    fn from_i8(v: i8) -> Self {
        v as f32
    }

    #[inline(always)]
    fn is_nan(self) -> bool {
        f32::is_nan(self)
    }

    #[inline(always)]
    fn floor(self) -> Self {
        f32::floor(self)
    }
    #[inline(always)]
    fn round(self) -> Self {
        f32::round(self)
    }
    #[inline(always)]
    fn trunc(self) -> Self {
        f32::trunc(self)
    }

    #[inline(always)]
    fn abs(self) -> Self {
        f32::abs(self)
    }
    #[inline(always)]
    fn sqrt(self) -> Self {
        f32::sqrt(self)
    }
    #[inline(always)]
    fn powf(self, exp: Self) -> Self {
        f32::powf(self, exp)
    }
    #[inline(always)]
    fn exp(self) -> Self {
        f32::exp(self)
    }
    #[inline(always)]
    fn ln(self) -> Self {
        f32::ln(self)
    }
    #[inline(always)]
    fn log10(self) -> Self {
        f32::log10(self)
    }
    #[inline(always)]
    fn sin(self) -> Self {
        f32::sin(self)
    }
    #[inline(always)]
    fn cos(self) -> Self {
        f32::cos(self)
    }
    #[inline(always)]
    fn tan(self) -> Self {
        f32::tan(self)
    }
    #[inline(always)]
    fn asin(self) -> Self {
        f32::asin(self)
    }
    #[inline(always)]
    fn acos(self) -> Self {
        f32::acos(self)
    }
    #[inline(always)]
    fn atan(self) -> Self {
        f32::atan(self)
    }
    #[inline(always)]
    fn rem_euclid(self, rhs: Self) -> Self {
        f32::rem_euclid(self, rhs)
    }

    #[inline(always)]
    fn approx_eq(self, other: Self) -> bool {
        float_cmp::approx_eq!(f32, self, other)
    }

    #[inline(always)]
    fn to_ordered(self) -> OrderedFloat<Self> {
        OrderedFloat(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_basic_ops() {
        let a: f64 = SimFloat::from_f64(3.14);
        // 3.14 != PI, so approx_eq should return false
        assert!(!a.approx_eq(std::f64::consts::PI));
        assert_eq!(<f64 as SimFloat>::zero(), 0.0);
        assert_eq!(<f64 as SimFloat>::one(), 1.0);
        assert!(<f64 as SimFloat>::nan().is_nan());
    }

    #[test]
    fn f32_basic_ops() {
        let a: f32 = SimFloat::from_f64(3.14);
        assert_eq!(<f32 as SimFloat>::zero(), 0.0f32);
        assert_eq!(<f32 as SimFloat>::one(), 1.0f32);
        assert!(<f32 as SimFloat>::nan().is_nan());
        assert!((a - 3.14f32).abs() < 0.001);
    }

    #[test]
    fn f64_approx_eq_matches_float_cmp() {
        let a: f64 = 1.0;
        let b: f64 = 1.0 + f64::EPSILON;
        // float_cmp with default ULP tolerance should consider these equal
        assert!(a.approx_eq(b));
    }

    #[test]
    fn f32_conversions_round_trip() {
        let original: f64 = 42.5;
        let narrow: f32 = SimFloat::from_f64(original);
        let wide: f64 = narrow.to_f64();
        assert!((wide - original).abs() < 1e-6);
    }
}
