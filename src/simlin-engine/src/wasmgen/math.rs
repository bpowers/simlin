// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: each public function emits a self-contained wasm helper
// `Function` (instruction sequence) for one transcendental. No I/O; the only
// side effect is in `#[cfg(test)]`, which executes the emitted helpers under the
// DLR-FT interpreter and compares against Rust `f64`.

//! Open-coded transcendental helpers for the wasm simulation backend.
//!
//! WebAssembly's MVP numeric instruction set provides `f64.sqrt`/`abs`/`floor`/
//! `ceil`/`trunc`/`nearest`/`min`/`max` and the arithmetic/compare ops, but *no*
//! transcendental instructions (`sin`/`cos`/`exp`/`ln`/...). The bytecode VM
//! reaches those through libm (`f64::sin` etc., `vm.rs::apply`). To stay a
//! self-contained module that imports no host math, this backend emits one wasm
//! helper function per transcendental, each built from range reduction plus a
//! polynomial/rational kernel over only the natively-available ops (plus
//! `i64.reinterpret_f64`/`f64.reinterpret_i64` for the exponent/mantissa bit
//! tricks `exp`/`ln` need).
//!
//! ## Accuracy bar
//!
//! These need not be bit-identical to libm. The bar is the `simulate.rs`
//! corpus tolerances (abs `2e-3` / rel `5e-6`, VDF `1%`): a model run through
//! this backend must clear the same comparison the VM clears. The kernels here
//! are chosen so each helper's worst-case error over its domain sits *far*
//! inside that bar (each emitter's rustdoc records the measured worst-case error
//! and the test that pins it); the slack absorbs any DLR-FT-vs-native rounding
//! drift. The per-helper unit tests assert against Rust `f64` with a documented
//! tolerance comfortably tighter than the corpus bar.
//!
//! ## Composition
//!
//! `tan = sin/cos`, `log10 = ln * (1/ln10)`, `asin = atan(x/sqrt(1-x^2))`,
//! `acos = pi/2 - asin`, and `pow(x, y) = exp(y * ln x)`. `pow` therefore
//! matches `f64::powf` only for a positive base; a negative base diverges
//! (`ln` of a negative is NaN). That is a documented limitation -- no corpus
//! model raises a negative base to a power -- so it is not chased here.
//!
//! ## Wiring
//!
//! Each emitter is pushed once by [`super::lower::build_helpers`], which records
//! the resulting function index in [`super::lower::HelperFns`]; the `Apply`
//! lowering (`lower.rs`, Phase 2 Task 4) and `Op2::Exp` (Task 3) reference a
//! helper by that index via `call`. No index is hard-coded.

use wasm_encoder::{Function, Instruction as Ins, ValType};

use super::lower::f64_const;

// ── Shared numeric constants (the kernels' magic numbers) ──────────────────

/// `ln(2)` (the exp/ln exponent <-> natural-log conversion).
const LN2: f64 = std::f64::consts::LN_2;
/// `1/ln(2) = log2(e)` (scales `x` to a base-2 exponent count in `exp`).
const LOG2E: f64 = std::f64::consts::LOG2_E;
/// `2/pi` (scales `x` to a count of `pi/2` quadrants in `sin`/`cos`).
const FRAC_2_PI: f64 = std::f64::consts::FRAC_2_PI;
/// `1/ln(10)` (converts a natural log to a base-10 log).
const INV_LN10: f64 = 1.0 / std::f64::consts::LN_10;

// IEEE-754 binary64 field geometry, used by the exp/ln bit tricks.
const EXP_BIAS: i64 = 1023;
const EXP_MASK: i64 = 0x7ff; // 11 exponent bits
const MANTISSA_BITS: i64 = 52;
const MANTISSA_MASK: i64 = 0x000f_ffff_ffff_ffff;
/// The exponent field of `1.0` (bias), pre-shifted into place: makes a raw
/// mantissa into a value in `[1, 2)`.
const ONE_EXP_FIELD: i64 = EXP_BIAS << MANTISSA_BITS;

// `exp` overflow/underflow thresholds (matching `f64::exp`): just past these,
// `exp(x)` rounds to `+inf` / `0`. Guarding here keeps the `2^k` exponent
// assembly inside the representable exponent range.
const EXP_OVERFLOW: f64 = 709.782_712_893_384;
const EXP_UNDERFLOW: f64 = -745.133_219_101_941_2;

// Cody-Waite three-part split of `pi/2` (the canonical fdlibm constants, each
// exactly representable in f64; `PIO2_1`'s low mantissa bits are zero so
// `x - k*PIO2_1` is exact). This keeps `r = x - k*(pi/2)` full-precision for
// `|k|` up to ~2^20 (sin/cos argument up to ~1e6).
const PIO2_1: f64 = 1.570_796_251_296_997; // pi/2, high ~33 bits
const PIO2_2: f64 = 7.549_789_415_861_596e-8; // next chunk
const PIO2_3: f64 = 5.390_302_529_957_765e-15; // remaining chunk

// atan reduction constants.
const SQRT3: f64 = 1.732_050_807_568_877_2;
const TAN_PI_12: f64 = 0.267_949_192_431_122_7; // 2 - sqrt(3) = tan(pi/12)

// ── Horner polynomial evaluation ────────────────────────────────────────────

/// Emit a Horner evaluation of `sum(coeffs[i] * v^i)` where `v` is the f64 in
/// `var_local`. Coefficients are given low-order-first; the emitter folds them
/// high-order-first (`acc = acc*v + c`), leaving the result on the stack.
///
/// `v` must already be materialized in `var_local` (a plain f64 local) because
/// Horner reads it once per term and the wasm operand stack is strict LIFO.
///
/// Shared with `super::alloc` (the `erfc_approx` Abramowitz-Stegun polynomial
/// folds with the identical `acc = acc*v + c` order, so reusing this keeps the
/// emitted op sequence bit-faithful to the Rust reference).
pub(crate) fn emit_horner(f: &mut Function, var_local: u32, coeffs: &[f64]) {
    // Start from the highest-order coefficient.
    let mut it = coeffs.iter().rev();
    let first = *it
        .next()
        .expect("polynomial needs at least one coefficient");
    f.instruction(&f64_const(first));
    for &c in it {
        // acc = acc * v + c
        f.instruction(&Ins::LocalGet(var_local));
        f.instruction(&Ins::F64Mul);
        f.instruction(&f64_const(c));
        f.instruction(&Ins::F64Add);
    }
}

// ── exp ─────────────────────────────────────────────────────────────────────

// `exp` local layout. Param 0 is `x`; the rest are scratch.
const EXP_X: u32 = 0;
const EXP_K: u32 = 1; // f64 k = round(x * log2e)
const EXP_R: u32 = 2; // f64 reduced argument r = x - k*ln2
const EXP_KI: u32 = 3; // i64 k as integer (the power of two to apply)

/// Taylor coefficients of `exp(r)` (`1/n!`, n = 0..=11). On `|r| <= ln2/2 ~=
/// 0.347` the degree-11 truncation is ~5e-15 relative -- far inside the bar.
const EXP_COEFFS: [f64; 12] = [
    1.0,
    1.0,
    1.0 / 2.0,
    1.0 / 6.0,
    1.0 / 24.0,
    1.0 / 120.0,
    1.0 / 720.0,
    1.0 / 5040.0,
    1.0 / 40320.0,
    1.0 / 362880.0,
    1.0 / 3628800.0,
    1.0 / 39916800.0,
];

/// Emit `exp(x: f64) -> f64`.
///
/// Range reduction `x = k*ln2 + r`, `|r| <= ln2/2`, then `exp(x) = 2^k *
/// exp(r)`: `exp(r)` is the Taylor poly ([`EXP_COEFFS`]), and `2^k` is applied
/// by adding `k` to the result's IEEE exponent field (`f64.reinterpret_i64`).
/// Guards: `NaN -> NaN`, `x > EXP_OVERFLOW -> +inf`, `x < EXP_UNDERFLOW -> 0`.
/// Because the post-guard `exp(r)` is always a normal number in `[0.70, 1.42]`
/// (exponent field `EXP_BIAS-1` or `EXP_BIAS`) and `k` is bounded by the
/// guards, the exponent-assembly path needs no subnormal special-case; an
/// out-of-range assembled exponent still saturates to `+inf`/`0` to be safe.
///
/// Worst-case error vs `f64::exp` over `[-700, 700]`: rel `~8e-14`. Pinned by
/// `exp_matches_f64`.
pub(crate) fn emit_exp() -> Function {
    // Locals (param 0 = x): f64 EXP_K(1)/EXP_R(2), i64 EXP_KI(3), then the
    // `emit_ldexp_exp_field` scratch f64 LDEXP_VAL(4) + i64 LDEXP_BITS(5)/
    // LDEXP_NEWEXP(6). Declaration order fixes these indices.
    let mut f = Function::new([
        (2, ValType::F64),
        (1, ValType::I64),
        (1, ValType::F64),
        (2, ValType::I64),
    ]);

    // NaN guard: x != x. If NaN, return x (which is NaN).
    f.instruction(&Ins::LocalGet(EXP_X));
    f.instruction(&Ins::LocalGet(EXP_X));
    f.instruction(&Ins::F64Ne);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&Ins::LocalGet(EXP_X));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // Overflow guard: x > EXP_OVERFLOW -> +inf.
    f.instruction(&Ins::LocalGet(EXP_X));
    f.instruction(&f64_const(EXP_OVERFLOW));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // Underflow guard: x < EXP_UNDERFLOW -> 0.
    f.instruction(&Ins::LocalGet(EXP_X));
    f.instruction(&f64_const(EXP_UNDERFLOW));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // k = nearest(x * log2e)
    f.instruction(&Ins::LocalGet(EXP_X));
    f.instruction(&f64_const(LOG2E));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Nearest);
    f.instruction(&Ins::LocalTee(EXP_K));
    // ki = trunc(k) as i64 (k is integer-valued; saturating is safe).
    f.instruction(&Ins::I64TruncSatF64S);
    f.instruction(&Ins::LocalSet(EXP_KI));

    // r = x - k*ln2
    f.instruction(&Ins::LocalGet(EXP_X));
    f.instruction(&Ins::LocalGet(EXP_K));
    f.instruction(&f64_const(LN2));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(EXP_R));

    // poly = exp(r) via Horner; leaves exp(r) on the stack.
    emit_horner(&mut f, EXP_R, &EXP_COEFFS);

    // Apply 2^k by adding ki to exp(r)'s exponent field.
    // bits = reinterpret(exp(r)); exp_field = (bits >> 52) & 0x7ff;
    // new_exp = exp_field + ki.
    emit_ldexp_exp_field(&mut f, EXP_KI);

    f.instruction(&Ins::End);
    f
}

// `emit_ldexp_exp_field` scratch (declared at the END of exp's locals so it does
// not collide with EXP_X/K/R/KI). The f64 `exp(r)` value is consumed off the
// stack into a fresh local.
const LDEXP_VAL: u32 = 4; // f64 exp(r)
const LDEXP_BITS: u32 = 5; // i64 raw bits of exp(r)
const LDEXP_NEWEXP: u32 = 6; // i64 candidate new exponent field

/// Consume the f64 on the stack (a *normal* value `e`, here always `exp(r) in
/// [0.70, 1.42]`) and push `e * 2^ki`, by adding `ki` (in `ki_local`) to `e`'s
/// IEEE exponent field. If the resulting exponent field is `>= EXP_MASK` push
/// `+inf` (e is positive here); if `<= 0` push `0`. Both saturations are
/// defensive: the `exp` over/underflow guards already bound `ki` so the in-range
/// branch is the one taken across the supported domain.
///
/// Requires three scratch locals declared by the caller: a f64 (`LDEXP_VAL`)
/// and two i64 (`LDEXP_BITS`, `LDEXP_NEWEXP`).
fn emit_ldexp_exp_field(f: &mut Function, ki_local: u32) {
    f.instruction(&Ins::LocalSet(LDEXP_VAL));

    // bits = reinterpret(val)
    f.instruction(&Ins::LocalGet(LDEXP_VAL));
    f.instruction(&Ins::I64ReinterpretF64);
    f.instruction(&Ins::LocalSet(LDEXP_BITS));

    // new_exp = ((bits >> 52) & 0x7ff) + ki
    f.instruction(&Ins::LocalGet(LDEXP_BITS));
    f.instruction(&Ins::I64Const(MANTISSA_BITS));
    f.instruction(&Ins::I64ShrU);
    f.instruction(&Ins::I64Const(EXP_MASK));
    f.instruction(&Ins::I64And);
    f.instruction(&Ins::LocalGet(ki_local));
    f.instruction(&Ins::I64Add);
    f.instruction(&Ins::LocalSet(LDEXP_NEWEXP));

    // if new_exp >= 0x7ff -> +inf
    f.instruction(&Ins::LocalGet(LDEXP_NEWEXP));
    f.instruction(&Ins::I64Const(EXP_MASK));
    f.instruction(&Ins::I64GeS);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::Else);
    // if new_exp <= 0 -> 0
    f.instruction(&Ins::LocalGet(LDEXP_NEWEXP));
    f.instruction(&Ins::I64Const(0));
    f.instruction(&Ins::I64LeS);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Else);
    // in range: rebuild bits with the new exponent field.
    // new_bits = (bits & ~(0x7ff << 52)) | (new_exp << 52)
    f.instruction(&Ins::LocalGet(LDEXP_BITS));
    f.instruction(&Ins::I64Const(!(EXP_MASK << MANTISSA_BITS)));
    f.instruction(&Ins::I64And);
    f.instruction(&Ins::LocalGet(LDEXP_NEWEXP));
    f.instruction(&Ins::I64Const(MANTISSA_BITS));
    f.instruction(&Ins::I64Shl);
    f.instruction(&Ins::I64Or);
    f.instruction(&Ins::F64ReinterpretI64);
    f.instruction(&Ins::End); // end inner if
    f.instruction(&Ins::End); // end outer if
}

// ── ln ─────────────────────────────────────────────────────────────────────

// `ln` local layout. Param 0 is `x`.
const LN_X: u32 = 0;
const LN_E: u32 = 1; // f64 exponent (after centering)
const LN_M: u32 = 2; // f64 mantissa in [sqrt(2)/2, sqrt(2))
const LN_S: u32 = 3; // f64 s = (m-1)/(m+1)
const LN_S2: u32 = 4; // f64 s^2
const LN_BITS: u32 = 5; // i64 raw bits of x

/// atanh-series coefficients `1/(2k+1)`, k = 0..=6, in `s^2`. On `|s| <= 0.1716`
/// (`m in [sqrt(2)/2, sqrt(2))`) the degree-13 truncation is ~1e-15 relative.
const LN_COEFFS: [f64; 7] = [
    1.0,
    1.0 / 3.0,
    1.0 / 5.0,
    1.0 / 7.0,
    1.0 / 9.0,
    1.0 / 11.0,
    1.0 / 13.0,
];

/// Emit `ln(x: f64) -> f64`.
///
/// Decompose `x = m * 2^e` with `m in [1, 2)` by reading the IEEE exponent and
/// mantissa fields; center `m` to `[sqrt(2)/2, sqrt(2))` (halve `m` and bump
/// `e` when `m > sqrt(2)`) so the atanh series in `s = (m-1)/(m+1)` converges
/// fast; `ln(x) = e*ln2 + 2*(s + s^3/3 + ...)`. Guards: `NaN or x < 0 -> NaN`,
/// `x == 0 -> -inf`, `+inf -> +inf`. Subnormal `x` (exponent field 0) is
/// normalized by scaling with `2^54` and subtracting 54 from `e`.
///
/// Worst-case error vs `f64::ln` over `[1e-10, 1e10]`: abs `~5e-13`. Pinned by
/// `ln_matches_f64`.
pub(crate) fn emit_ln() -> Function {
    // Locals (param 0 = x): f64 LN_E(1)/LN_M(2)/LN_S(3)/LN_S2(4), i64 LN_BITS(5).
    let mut f = Function::new([(4, ValType::F64), (1, ValType::I64)]);

    // NaN-or-negative guard: !(x >= 0) (true for NaN and x<0) -> NaN.
    // x < 0 -> NaN; NaN handled by the same (x != x) check folded in below.
    // Use: if (x < 0) | (x != x) -> return NaN.
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&Ins::F64Ne);
    f.instruction(&Ins::I32Or);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // x == 0 -> -inf.
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&f64_const(f64::NEG_INFINITY));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // +inf -> +inf.
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // Decompose. Handle subnormal (exponent field == 0) by scaling up first.
    // if ((reinterpret(x) >> 52) & 0x7ff) == 0 { x *= 2^54; e_adjust = -54 }
    // We fold the adjust into LN_E after extracting the (now-normal) fields.
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&Ins::I64ReinterpretF64);
    f.instruction(&Ins::I64Const(MANTISSA_BITS));
    f.instruction(&Ins::I64ShrU);
    f.instruction(&Ins::I64Const(EXP_MASK));
    f.instruction(&Ins::I64And);
    f.instruction(&Ins::I64Eqz); // exponent field == 0 (subnormal/zero; zero already handled)
    f.instruction(&Ins::If(wasm_encoder::BlockType::Result(ValType::F64)));
    // subnormal: x_scaled = x * 2^54, and remember -54 in LN_E.
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&f64_const(f64::from_bits(((EXP_BIAS + 54) as u64) << 52)));
    f.instruction(&Ins::F64Mul);
    f.instruction(&f64_const(-54.0));
    f.instruction(&Ins::LocalSet(LN_E));
    f.instruction(&Ins::Else);
    // normal: x unchanged, e adjust 0.
    f.instruction(&Ins::LocalGet(LN_X));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(LN_E));
    f.instruction(&Ins::End);
    // stack: [x_norm]. bits = reinterpret(x_norm).
    f.instruction(&Ins::I64ReinterpretF64);
    f.instruction(&Ins::LocalSet(LN_BITS));

    // m = mantissa-with-exponent-of-1.0 (value in [1,2)).
    f.instruction(&Ins::LocalGet(LN_BITS));
    f.instruction(&Ins::I64Const(MANTISSA_MASK));
    f.instruction(&Ins::I64And);
    f.instruction(&Ins::I64Const(ONE_EXP_FIELD));
    f.instruction(&Ins::I64Or);
    f.instruction(&Ins::F64ReinterpretI64);
    f.instruction(&Ins::LocalSet(LN_M));

    // e += (exponent_field - bias).
    f.instruction(&Ins::LocalGet(LN_E));
    f.instruction(&Ins::LocalGet(LN_BITS));
    f.instruction(&Ins::I64Const(MANTISSA_BITS));
    f.instruction(&Ins::I64ShrU);
    f.instruction(&Ins::I64Const(EXP_MASK));
    f.instruction(&Ins::I64And);
    f.instruction(&Ins::I64Const(EXP_BIAS));
    f.instruction(&Ins::I64Sub);
    f.instruction(&Ins::F64ConvertI64S);
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(LN_E));

    // Center: if m > sqrt(2) { m *= 0.5; e += 1 }.
    f.instruction(&Ins::LocalGet(LN_M));
    f.instruction(&f64_const(std::f64::consts::SQRT_2));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&Ins::LocalGet(LN_M));
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(LN_M));
    f.instruction(&Ins::LocalGet(LN_E));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(LN_E));
    f.instruction(&Ins::End);

    // s = (m - 1) / (m + 1); s2 = s*s.
    f.instruction(&Ins::LocalGet(LN_M));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalGet(LN_M));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalTee(LN_S));
    f.instruction(&Ins::LocalGet(LN_S));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(LN_S2));

    // ln(m) = 2 * s * poly(s2); result = e*ln2 + ln(m).
    f.instruction(&Ins::LocalGet(LN_E));
    f.instruction(&f64_const(LN2));
    f.instruction(&Ins::F64Mul);
    f.instruction(&f64_const(2.0));
    f.instruction(&Ins::LocalGet(LN_S));
    f.instruction(&Ins::F64Mul);
    emit_horner(&mut f, LN_S2, &LN_COEFFS);
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Add);

    f.instruction(&Ins::End);
    f
}

// ── sin / cos (shared kernel) ────────────────────────────────────────────────

// sin/cos local layout. Param 0 is `x`.
const SC_X: u32 = 0;
const SC_K: u32 = 1; // f64 quadrant count
const SC_R: u32 = 2; // f64 reduced argument in [-pi/4, pi/4]
const SC_R2: u32 = 3; // f64 r^2
const SC_SR: u32 = 4; // f64 sin(r)
const SC_CR: u32 = 5; // f64 cos(r)
const SC_KQ: u32 = 6; // i64 quadrant index k mod 4

/// `sin(r)/r` Taylor coefficients in `r^2` (so the series is `r * poly(r^2)`):
/// `(-1)^n / (2n+1)!`, n = 0..=5 (through `r^11`).
const SIN_COEFFS: [f64; 6] = [
    1.0,
    -1.0 / 6.0,
    1.0 / 120.0,
    -1.0 / 5040.0,
    1.0 / 362880.0,
    -1.0 / 39916800.0,
];

/// `cos(r)` Taylor coefficients in `r^2`: `(-1)^n / (2n)!`, n = 0..=5 (through
/// `r^10`).
const COS_COEFFS: [f64; 6] = [
    1.0,
    -1.0 / 2.0,
    1.0 / 24.0,
    -1.0 / 720.0,
    1.0 / 40320.0,
    -1.0 / 3628800.0,
];

/// Emit the shared sin/cos body. `want_sin` selects which result the function
/// returns; both `sin(r)` and `cos(r)` are computed (cheap) and the quadrant
/// `k mod 4` selects/sign-flips the right one, exactly mirroring the kernel
/// the prototype validated.
fn emit_sincos(want_sin: bool) -> Function {
    // Locals (param 0 = x): f64 SC_K(1)/SC_R(2)/SC_R2(3)/SC_SR(4)/SC_CR(5),
    // i64 SC_KQ(6).
    let mut f = Function::new([(5, ValType::F64), (1, ValType::I64)]);

    // NaN/inf guard: if !(|x| < +inf) return NaN. (|x| < inf is false for NaN
    // and for +-inf.)
    f.instruction(&Ins::LocalGet(SC_X));
    f.instruction(&Ins::F64Abs);
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(wasm_encoder::BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // k = nearest(x * 2/pi); kq = k mod 4 (normalized to 0..=3).
    f.instruction(&Ins::LocalGet(SC_X));
    f.instruction(&f64_const(FRAC_2_PI));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Nearest);
    f.instruction(&Ins::LocalTee(SC_K));
    // kq = ((k as i64) % 4 + 4) % 4
    f.instruction(&Ins::I64TruncSatF64S);
    f.instruction(&Ins::I64Const(4));
    f.instruction(&Ins::I64RemS);
    f.instruction(&Ins::I64Const(4));
    f.instruction(&Ins::I64Add);
    f.instruction(&Ins::I64Const(4));
    f.instruction(&Ins::I64RemS);
    f.instruction(&Ins::LocalSet(SC_KQ));

    // r = ((x - k*PIO2_1) - k*PIO2_2) - k*PIO2_3.
    f.instruction(&Ins::LocalGet(SC_X));
    f.instruction(&Ins::LocalGet(SC_K));
    f.instruction(&f64_const(PIO2_1));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalGet(SC_K));
    f.instruction(&f64_const(PIO2_2));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalGet(SC_K));
    f.instruction(&f64_const(PIO2_3));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalTee(SC_R));
    // r2 = r*r
    f.instruction(&Ins::LocalGet(SC_R));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(SC_R2));

    // sr = r * poly_sin(r2)
    f.instruction(&Ins::LocalGet(SC_R));
    emit_horner(&mut f, SC_R2, &SIN_COEFFS);
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(SC_SR));
    // cr = poly_cos(r2)
    emit_horner(&mut f, SC_R2, &COS_COEFFS);
    f.instruction(&Ins::LocalSet(SC_CR));

    // Quadrant select. For sin: kq 0->sr, 1->cr, 2->-sr, 3->-cr.
    // For cos: kq 0->cr, 1->-sr, 2->-cr, 3->sr.
    // Emit a 4-way nested select keyed on kq.
    emit_quadrant_select(&mut f, want_sin);

    f.instruction(&Ins::End);
    f
}

/// Push the quadrant-selected result for sin (`want_sin`) or cos. Reads
/// `SC_SR`/`SC_CR`/`SC_KQ`. Implemented as three chained `select`s, keyed on
/// `kq != n`, avoiding branches.
///
/// wasm `select` pops `[a, b, cond]` and yields the *deeper* operand `a` when
/// `cond != 0`, else the shallower `b`. The running result (the default for
/// `kq == 0`, refined by earlier iterations) is already on the stack as the
/// deeper operand; pushing the override `q_n` above it and selecting on
/// `kq != n` keeps the running value when `kq != n` and switches to `q_n`
/// otherwise.
fn emit_quadrant_select(f: &mut Function, want_sin: bool) {
    // The four results per quadrant (one `push_*` emitter each).
    let [q0, q1, q2, q3]: [PushFn; 4] = if want_sin {
        [push_sr, push_cr, push_neg_sr, push_neg_cr]
    } else {
        [push_cr, push_neg_sr, push_neg_cr, push_sr]
    };

    q0(f); // running result, default for kq == 0
    for (n, push_q) in [(1i64, q1), (2, q2), (3, q3)] {
        push_q(f); // override candidate (shallower)
        push_kq_ne(f, n); // cond: keep the running (deeper) value when kq != n
        f.instruction(&Ins::Select);
    }
}

/// An emitter that pushes one quadrant result (`sr`/`cr`/`-sr`/`-cr`) onto the
/// stack from the precomputed `SC_SR`/`SC_CR` locals.
type PushFn = fn(&mut Function);

fn push_sr(f: &mut Function) {
    f.instruction(&Ins::LocalGet(SC_SR));
}
fn push_cr(f: &mut Function) {
    f.instruction(&Ins::LocalGet(SC_CR));
}
fn push_neg_sr(f: &mut Function) {
    f.instruction(&Ins::LocalGet(SC_SR));
    f.instruction(&Ins::F64Neg);
}
fn push_neg_cr(f: &mut Function) {
    f.instruction(&Ins::LocalGet(SC_CR));
    f.instruction(&Ins::F64Neg);
}
/// Push i32 `1` when `SC_KQ != n`, else `0`. Used as the `select` condition so
/// the deeper (running) operand is kept when `kq != n`.
fn push_kq_ne(f: &mut Function, n: i64) {
    f.instruction(&Ins::LocalGet(SC_KQ));
    f.instruction(&Ins::I64Const(n));
    f.instruction(&Ins::I64Ne);
}

/// Emit `sin(x: f64) -> f64`. Worst-case error vs `f64::sin` over `[-1e6, 1e6]`:
/// abs `~1.2e-10`. Pinned by `sin_matches_f64`.
pub(crate) fn emit_sin() -> Function {
    emit_sincos(true)
}

/// Emit `cos(x: f64) -> f64`. Worst-case error vs `f64::cos` over `[-1e6, 1e6]`:
/// abs `~1.2e-10`. Pinned by `cos_matches_f64`.
pub(crate) fn emit_cos() -> Function {
    emit_sincos(false)
}

// ── tan = sin / cos ──────────────────────────────────────────────────────────

const TAN_X: u32 = 0;

/// Emit `tan(x: f64) -> f64` as `sin(x) / cos(x)` by `call`ing the sin/cos
/// helpers. Worst-case relative error over `[-1.5, 1.5]` (away from the poles):
/// `~1.5e-10`. Pinned by `tan_matches_f64`.
///
/// `sin_idx`/`cos_idx` are the module function indices of [`emit_sin`] /
/// [`emit_cos`].
pub(crate) fn emit_tan(sin_idx: u32, cos_idx: u32) -> Function {
    let mut f = Function::new([]);
    f.instruction(&Ins::LocalGet(TAN_X));
    f.instruction(&Ins::Call(sin_idx));
    f.instruction(&Ins::LocalGet(TAN_X));
    f.instruction(&Ins::Call(cos_idx));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::End);
    f
}

// ── atan ─────────────────────────────────────────────────────────────────────

// atan local layout. Param 0 is `x`.
const AT_X: u32 = 0;
const AT_AX: u32 = 1; // f64 |x|
const AT_Z: u32 = 2; // f64 reduced argument
const AT_Z2: u32 = 3; // f64 z^2
const AT_RECIP: u32 = 4; // i32 1 if |x| > 1
const AT_SHIFT: u32 = 5; // i32 1 if the pi/6 shift was applied
const AT_SIGN: u32 = 6; // f64 sign of x (+-1)

/// `atan(z)/z` Taylor coefficients in `z^2`: `(-1)^n / (2n+1)`, n = 0..=6
/// (through `z^13`). On `|z| <= tan(pi/12) ~= 0.268` the truncation is
/// ~1e-10 relative.
const ATAN_COEFFS: [f64; 7] = [
    1.0,
    -1.0 / 3.0,
    1.0 / 5.0,
    -1.0 / 7.0,
    1.0 / 9.0,
    -1.0 / 11.0,
    1.0 / 13.0,
];

/// Emit `atan(x: f64) -> f64`.
///
/// Two-stage range reduction to a small argument:
/// 1. `|x| > 1` -> `atan(|x|) = pi/2 - atan(1/|x|)` (so `z0 in [0, 1]`).
/// 2. `z0 > tan(pi/12)` -> `atan(z0) = pi/6 + atan((z0*sqrt3 - 1)/(sqrt3 + z0))`
///    (so the poly argument `z in [-(2-sqrt3), 2-sqrt3]`).
///
/// then `atan(z) = z * poly(z^2)`, undoing the shifts and applying the sign.
/// `+-inf -> +-pi/2`, `NaN -> NaN` (the poly of a NaN is NaN, and the
/// reductions preserve it). Worst-case error vs `f64::atan` over `[-1000,
/// 1000]`: rel `~6e-10`. Pinned by `atan_matches_f64`.
pub(crate) fn emit_atan() -> Function {
    use wasm_encoder::BlockType;
    // Locals (param 0 = x): f64 AT_AX(1)/AT_Z(2)/AT_Z2(3), i32 AT_RECIP(4)/
    // AT_SHIFT(5), f64 AT_SIGN(6).
    let mut f = Function::new([(3, ValType::F64), (2, ValType::I32), (1, ValType::F64)]);

    // +inf -> pi/2, -inf -> -pi/2 (handled first so the reciprocal 1/inf = 0
    // path is not relied upon).
    f.instruction(&Ins::LocalGet(AT_X));
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(std::f64::consts::FRAC_PI_2));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);
    f.instruction(&Ins::LocalGet(AT_X));
    f.instruction(&f64_const(f64::NEG_INFINITY));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(-std::f64::consts::FRAC_PI_2));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // sign = x < 0 ? -1 : 1 ; ax = |x|.
    f.instruction(&f64_const(-1.0));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(AT_X));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalSet(AT_SIGN));
    f.instruction(&Ins::LocalGet(AT_X));
    f.instruction(&Ins::F64Abs);
    f.instruction(&Ins::LocalSet(AT_AX));

    // recip = ax > 1 ; z0 = recip ? 1/ax : ax.
    f.instruction(&Ins::LocalGet(AT_AX));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::LocalSet(AT_RECIP));
    // z0 = select(1/ax, ax, recip)
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(AT_AX));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalGet(AT_AX));
    f.instruction(&Ins::LocalGet(AT_RECIP));
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalSet(AT_Z));

    // shift = z0 > tan(pi/12) ; z = shift ? (z0*sqrt3 - 1)/(sqrt3 + z0) : z0.
    f.instruction(&Ins::LocalGet(AT_Z));
    f.instruction(&f64_const(TAN_PI_12));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::LocalSet(AT_SHIFT));
    // shifted = (z0*sqrt3 - 1)/(sqrt3 + z0)
    f.instruction(&Ins::LocalGet(AT_Z));
    f.instruction(&f64_const(SQRT3));
    f.instruction(&Ins::F64Mul);
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Sub);
    f.instruction(&f64_const(SQRT3));
    f.instruction(&Ins::LocalGet(AT_Z));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Div);
    // select(shifted, z0, shift)
    f.instruction(&Ins::LocalGet(AT_Z));
    f.instruction(&Ins::LocalGet(AT_SHIFT));
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalTee(AT_Z));
    // z2 = z*z
    f.instruction(&Ins::LocalGet(AT_Z));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(AT_Z2));

    // at = z * poly(z2)
    f.instruction(&Ins::LocalGet(AT_Z));
    emit_horner(&mut f, AT_Z2, &ATAN_COEFFS);
    f.instruction(&Ins::F64Mul);
    // at += shift ? pi/6 : 0
    f.instruction(&f64_const(std::f64::consts::FRAC_PI_6));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalGet(AT_SHIFT));
    f.instruction(&Ins::Select);
    f.instruction(&Ins::F64Add);
    // at = recip ? pi/2 - at : at
    // compute (pi/2 - at) and select.
    f.instruction(&Ins::LocalSet(AT_Z)); // reuse AT_Z to hold the running atan value
    f.instruction(&f64_const(std::f64::consts::FRAC_PI_2));
    f.instruction(&Ins::LocalGet(AT_Z));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalGet(AT_Z));
    f.instruction(&Ins::LocalGet(AT_RECIP));
    f.instruction(&Ins::Select);
    // * sign
    f.instruction(&Ins::LocalGet(AT_SIGN));
    f.instruction(&Ins::F64Mul);

    f.instruction(&Ins::End);
    f
}

// ── asin / acos ───────────────────────────────────────────────────────────────

const AS_X: u32 = 0;

/// Emit `asin(x: f64) -> f64` as `atan(x / sqrt(1 - x^2))` with endpoint and
/// domain handling: `|x| > 1 -> NaN`, `x == 1 -> pi/2`, `x == -1 -> -pi/2`
/// (at the endpoints `sqrt(1-x^2)=0` would divide by zero). `NaN -> NaN`.
/// Worst-case error vs `f64::asin` over `[-1, 1]`: abs `~1.6e-10`. Pinned by
/// `asin_matches_f64`. `atan_idx` is [`emit_atan`]'s module function index.
pub(crate) fn emit_asin(atan_idx: u32) -> Function {
    use wasm_encoder::BlockType;
    let mut f = Function::new([]);

    // |x| > 1 -> NaN (also catches nothing for NaN; NaN handled by falling
    // through to the poly which yields NaN, but be explicit:)
    // if (x > 1) | (x < -1) -> NaN
    f.instruction(&Ins::LocalGet(AS_X));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::LocalGet(AS_X));
    f.instruction(&f64_const(-1.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::I32Or);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // x == 1 -> pi/2.
    f.instruction(&Ins::LocalGet(AS_X));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(std::f64::consts::FRAC_PI_2));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);
    // x == -1 -> -pi/2.
    f.instruction(&Ins::LocalGet(AS_X));
    f.instruction(&f64_const(-1.0));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(-std::f64::consts::FRAC_PI_2));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // atan(x / sqrt(1 - x*x))
    f.instruction(&Ins::LocalGet(AS_X));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(AS_X));
    f.instruction(&Ins::LocalGet(AS_X));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Sqrt);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::Call(atan_idx));
    f.instruction(&Ins::End);
    f
}

const AC_X: u32 = 0;

/// Emit `acos(x: f64) -> f64` as `pi/2 - asin(x)`. Domain `|x| > 1 -> NaN`
/// (inherited from asin), `NaN -> NaN`. Worst-case error vs `f64::acos` over
/// `[-1, 1]`: abs `~1.6e-10`. Pinned by `acos_matches_f64`. `asin_idx` is
/// [`emit_asin`]'s module function index.
pub(crate) fn emit_acos(asin_idx: u32) -> Function {
    let mut f = Function::new([]);
    f.instruction(&f64_const(std::f64::consts::FRAC_PI_2));
    f.instruction(&Ins::LocalGet(AC_X));
    f.instruction(&Ins::Call(asin_idx));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::End);
    f
}

// ── log10 = ln * (1/ln10) ─────────────────────────────────────────────────────

const LOG10_X: u32 = 0;

/// Emit `log10(x: f64) -> f64` as `ln(x) * (1/ln10)`. Inherits `ln`'s domain
/// handling (`x < 0 -> NaN`, `x == 0 -> -inf`). Worst-case error vs
/// `f64::log10` over `[1e-10, 1e10]`: abs `~2e-13`. Pinned by
/// `log10_matches_f64`. `ln_idx` is [`emit_ln`]'s module function index.
pub(crate) fn emit_log10(ln_idx: u32) -> Function {
    let mut f = Function::new([]);
    f.instruction(&Ins::LocalGet(LOG10_X));
    f.instruction(&Ins::Call(ln_idx));
    f.instruction(&f64_const(INV_LN10));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::End);
    f
}

// ── pow = exp(y * ln x) ────────────────────────────────────────────────────────

const POW_X: u32 = 0;
const POW_Y: u32 = 1;

/// Emit `pow(x: f64, y: f64) -> f64` as `exp(y * ln x)`.
///
/// Matches `f64::powf` for a positive base `x`. Special cases mirrored from
/// `powf`: `y == 0 -> 1` (including `pow(anything, 0) == 1`), `x == 1 -> 1`.
/// A negative base yields NaN (`ln` of a negative is NaN) -- this is the
/// documented limitation; no corpus model raises a negative base to a power.
/// Worst-case relative error over `x in [0.01, 100]`, `y in [-5, 5]`:
/// `~2.3e-12`. Pinned by `pow_matches_f64`. `exp_idx`/`ln_idx` are the module
/// function indices of [`emit_exp`] / [`emit_ln`].
pub(crate) fn emit_pow(exp_idx: u32, ln_idx: u32) -> Function {
    use wasm_encoder::BlockType;
    let mut f = Function::new([]);

    // y == 0 -> 1.
    f.instruction(&Ins::LocalGet(POW_Y));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);
    // x == 1 -> 1.
    f.instruction(&Ins::LocalGet(POW_X));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // exp(y * ln(x))
    f.instruction(&Ins::LocalGet(POW_Y));
    f.instruction(&Ins::LocalGet(POW_X));
    f.instruction(&Ins::Call(ln_idx));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::Call(exp_idx));
    f.instruction(&Ins::End);
    f
}

#[cfg(test)]
mod tests {
    use super::super::lower::build_helpers;
    use checked::Store;
    use wasm::validate;
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction,
        MemorySection, MemoryType, Module, TypeSection, ValType,
    };

    /// Which transcendental helper a test module exports as `f`.
    #[derive(Clone, Copy)]
    enum Which {
        Exp,
        Ln,
        Sin,
        Cos,
        Tan,
        Atan,
        Asin,
        Acos,
        Log10,
        Pow,
    }

    /// Resolve a [`Which`] to its function index in the assembled helper table.
    fn helper_index(which: Which) -> u32 {
        let h = build_helpers().fns;
        match which {
            Which::Exp => h.exp,
            Which::Ln => h.ln,
            Which::Sin => h.sin,
            Which::Cos => h.cos,
            Which::Tan => h.tan,
            Which::Atan => h.atan,
            Which::Asin => h.asin,
            Which::Acos => h.acos,
            Which::Log10 => h.log10,
            Which::Pow => h.pow,
        }
    }

    /// Build a module containing *every* helper body (so inter-helper `call`s
    /// resolve) plus a thin exported wrapper `f` that forwards to the
    /// helper-under-test. Unary helpers export `f(x: f64) -> f64`; `pow` exports
    /// `f(x: f64, y: f64) -> f64`. Mirrors `lower.rs`'s production assembly:
    /// helpers occupy function indices `0..N`, the wrapper follows at `N`.
    fn build_helper_module(which: Which) -> Vec<u8> {
        let helpers = build_helpers();
        let n_helpers = helpers.functions.len() as u32;
        let target = helper_index(which);
        let binary = matches!(which, Which::Pow);

        let mut module = Module::new();

        // Type 0 is the wrapper's signature; each helper's signature follows.
        let mut types = TypeSection::new();
        if binary {
            types
                .ty()
                .function([ValType::F64, ValType::F64], [ValType::F64]);
        } else {
            types.ty().function([ValType::F64], [ValType::F64]);
        }
        for hf in &helpers.functions {
            types.ty().function(hf.params.clone(), hf.results.clone());
        }
        module.section(&types);

        let mut functions = FunctionSection::new();
        for (i, _) in helpers.functions.iter().enumerate() {
            functions.function(1 + i as u32);
        }
        functions.function(0);
        module.section(&functions);

        // The GF lookup helpers (`super::lookup`) `f64.load` from memory 0, so
        // a module that includes every helper body must declare a memory even
        // though the transcendental wrappers here never touch it.
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);

        let mut exports = ExportSection::new();
        exports.export("f", ExportKind::Func, n_helpers);
        module.section(&exports);

        let mut code = CodeSection::new();
        for hf in &helpers.functions {
            code.function(&hf.body);
        }
        let mut wrapper = Function::new([]);
        wrapper.instruction(&Instruction::LocalGet(0));
        if binary {
            wrapper.instruction(&Instruction::LocalGet(1));
        }
        wrapper.instruction(&Instruction::Call(target));
        wrapper.instruction(&Instruction::End);
        code.function(&wrapper);
        module.section(&code);

        module.finish()
    }

    /// Run a unary helper on `x` under the DLR-FT interpreter. The module is
    /// (re)built per call; the samples are deliberately small (a few hundred
    /// points each) so this stays well under the per-test time budget.
    fn run_unary(which: Which, x: f64) -> f64 {
        let bytes = build_helper_module(which);
        let info = validate(&bytes).expect("helper module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("helper module must instantiate")
            .module_addr;
        let f = store
            .instance_export(module, "f")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(f64,), f64>(f, (x,))
            .expect("invocation must succeed")
    }

    /// Run `pow(x, y)` under the interpreter.
    fn run_pow(x: f64, y: f64) -> f64 {
        let bytes = build_helper_module(Which::Pow);
        let info = validate(&bytes).expect("pow module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("pow module must instantiate")
            .module_addr;
        let f = store
            .instance_export(module, "f")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(f64, f64), f64>(f, (x, y))
            .expect("invocation must succeed")
    }

    /// A linear sample of `n+1` points across `[lo, hi]` inclusive.
    fn linspace(lo: f64, hi: f64, n: usize) -> Vec<f64> {
        (0..=n)
            .map(|i| lo + (hi - lo) * (i as f64) / (n as f64))
            .collect()
    }

    /// Assert `got` matches `want` within absolute *or* relative tolerance,
    /// propagating the float specials the way the kernels are documented to.
    fn assert_close(name: &str, x: f64, got: f64, want: f64, abs_tol: f64, rel_tol: f64) {
        if want.is_nan() {
            assert!(got.is_nan(), "{name}({x}): expected NaN, got {got}");
            return;
        }
        assert!(!got.is_nan(), "{name}({x}): got NaN, expected {want}");
        if want.is_infinite() {
            assert_eq!(got, want, "{name}({x}): expected {want}, got {got}");
            return;
        }
        let abs = (got - want).abs();
        let rel = if want != 0.0 { abs / want.abs() } else { abs };
        assert!(
            abs <= abs_tol || rel <= rel_tol,
            "{name}({x}): got {got}, want {want} (abs {abs:.3e}, rel {rel:.3e})",
        );
    }

    // The corpus bar is abs 2e-3 / rel 5e-6. Every per-helper tolerance below is
    // far inside that, leaving ample slack for DLR-FT-vs-native rounding drift.

    // ── exp ───────────────────────────────────────────────────────────────

    #[test]
    fn exp_matches_f64() {
        // Anchor values exercise the wrapper end-to-end.
        assert_eq!(run_unary(Which::Exp, 0.0), 1.0);
        assert_close(
            "exp",
            1.0,
            run_unary(Which::Exp, 1.0),
            std::f64::consts::E,
            0.0,
            1e-12,
        );
        // Dense sweep across the representable exponent range.
        for x in linspace(-700.0, 700.0, 300) {
            assert_close("exp", x, run_unary(Which::Exp, x), x.exp(), 0.0, 1e-12);
        }
        // Edge / special cases.
        assert!(run_unary(Which::Exp, f64::NAN).is_nan());
        assert_eq!(run_unary(Which::Exp, f64::INFINITY), f64::INFINITY);
        assert_eq!(run_unary(Which::Exp, f64::NEG_INFINITY), 0.0);
        assert_eq!(run_unary(Which::Exp, 720.0), f64::INFINITY); // overflow
        assert_eq!(run_unary(Which::Exp, -750.0), 0.0); // underflow
    }

    // ── ln ────────────────────────────────────────────────────────────────

    #[test]
    fn ln_matches_f64() {
        assert_eq!(run_unary(Which::Ln, 1.0), 0.0);
        assert_close(
            "ln",
            std::f64::consts::E,
            run_unary(Which::Ln, std::f64::consts::E),
            1.0,
            1e-12,
            1e-12,
        );
        // Geometric sweep over many decades (where ln is interesting).
        for e in linspace(-300.0, 300.0, 300) {
            let x = 10f64.powf(e / 30.0);
            assert_close("ln", x, run_unary(Which::Ln, x), x.ln(), 1e-12, 1e-11);
        }
        // Subnormal input (exercises the 2^54 normalization path).
        let sub = f64::from_bits(1);
        assert_close("ln", sub, run_unary(Which::Ln, sub), sub.ln(), 1e-9, 1e-12);
        // Domain edges.
        assert_eq!(run_unary(Which::Ln, 0.0), f64::NEG_INFINITY);
        assert!(run_unary(Which::Ln, -1.0).is_nan());
        assert!(run_unary(Which::Ln, f64::NAN).is_nan());
        assert_eq!(run_unary(Which::Ln, f64::INFINITY), f64::INFINITY);
    }

    // ── sin / cos ───────────────────────────────────────────────────────────

    #[test]
    fn sin_matches_f64() {
        assert_eq!(run_unary(Which::Sin, 0.0), 0.0);
        for x in linspace(-100.0, 100.0, 400) {
            assert_close("sin", x, run_unary(Which::Sin, x), x.sin(), 1e-9, 1e-9);
        }
        // A few large arguments to exercise the Cody-Waite reduction.
        for &x in &[1.0e3, -1.0e4, 1.0e5, -650_400.0] {
            assert_close("sin", x, run_unary(Which::Sin, x), x.sin(), 1e-8, 1e-7);
        }
        assert!(run_unary(Which::Sin, f64::NAN).is_nan());
        assert!(run_unary(Which::Sin, f64::INFINITY).is_nan());
    }

    #[test]
    fn cos_matches_f64() {
        assert_eq!(run_unary(Which::Cos, 0.0), 1.0);
        for x in linspace(-100.0, 100.0, 400) {
            assert_close("cos", x, run_unary(Which::Cos, x), x.cos(), 1e-9, 1e-9);
        }
        for &x in &[1.0e3, -1.0e4, 1.0e5, -650_400.0] {
            assert_close("cos", x, run_unary(Which::Cos, x), x.cos(), 1e-8, 1e-7);
        }
        assert!(run_unary(Which::Cos, f64::NAN).is_nan());
        assert!(run_unary(Which::Cos, f64::NEG_INFINITY).is_nan());
    }

    // ── tan ─────────────────────────────────────────────────────────────────

    #[test]
    fn tan_matches_f64() {
        assert_eq!(run_unary(Which::Tan, 0.0), 0.0);
        // Stay away from the +-pi/2 poles where the function is ill-conditioned.
        for x in linspace(-1.4, 1.4, 400) {
            assert_close("tan", x, run_unary(Which::Tan, x), x.tan(), 1e-9, 1e-8);
        }
        assert!(run_unary(Which::Tan, f64::NAN).is_nan());
    }

    // ── atan ────────────────────────────────────────────────────────────────

    #[test]
    fn atan_matches_f64() {
        assert_eq!(run_unary(Which::Atan, 0.0), 0.0);
        for x in linspace(-1000.0, 1000.0, 400) {
            assert_close("atan", x, run_unary(Which::Atan, x), x.atan(), 1e-9, 1e-9);
        }
        // Dense small region around the two reduction breakpoints (1 and
        // tan(pi/12)).
        for x in linspace(-2.0, 2.0, 200) {
            assert_close("atan", x, run_unary(Which::Atan, x), x.atan(), 1e-9, 1e-9);
        }
        assert_close(
            "atan",
            f64::INFINITY,
            run_unary(Which::Atan, f64::INFINITY),
            std::f64::consts::FRAC_PI_2,
            1e-12,
            0.0,
        );
        assert_close(
            "atan",
            f64::NEG_INFINITY,
            run_unary(Which::Atan, f64::NEG_INFINITY),
            -std::f64::consts::FRAC_PI_2,
            1e-12,
            0.0,
        );
        assert!(run_unary(Which::Atan, f64::NAN).is_nan());
    }

    // ── asin / acos ───────────────────────────────────────────────────────────

    #[test]
    fn asin_matches_f64() {
        for x in linspace(-1.0, 1.0, 400) {
            assert_close("asin", x, run_unary(Which::Asin, x), x.asin(), 1e-9, 1e-9);
        }
        // Exact endpoints.
        assert_close(
            "asin",
            1.0,
            run_unary(Which::Asin, 1.0),
            std::f64::consts::FRAC_PI_2,
            1e-12,
            0.0,
        );
        assert_close(
            "asin",
            -1.0,
            run_unary(Which::Asin, -1.0),
            -std::f64::consts::FRAC_PI_2,
            1e-12,
            0.0,
        );
        // Out of domain.
        assert!(run_unary(Which::Asin, 1.5).is_nan());
        assert!(run_unary(Which::Asin, -1.5).is_nan());
        assert!(run_unary(Which::Asin, f64::NAN).is_nan());
    }

    #[test]
    fn acos_matches_f64() {
        for x in linspace(-1.0, 1.0, 400) {
            assert_close("acos", x, run_unary(Which::Acos, x), x.acos(), 1e-9, 1e-9);
        }
        assert_close("acos", 1.0, run_unary(Which::Acos, 1.0), 0.0, 1e-9, 0.0);
        assert_close(
            "acos",
            -1.0,
            run_unary(Which::Acos, -1.0),
            std::f64::consts::PI,
            1e-12,
            1e-12,
        );
        assert!(run_unary(Which::Acos, 1.5).is_nan());
        assert!(run_unary(Which::Acos, f64::NAN).is_nan());
    }

    // ── log10 ──────────────────────────────────────────────────────────────

    #[test]
    fn log10_matches_f64() {
        assert_close(
            "log10",
            1000.0,
            run_unary(Which::Log10, 1000.0),
            3.0,
            1e-12,
            1e-12,
        );
        for e in linspace(-300.0, 300.0, 300) {
            let x = 10f64.powf(e / 30.0);
            assert_close(
                "log10",
                x,
                run_unary(Which::Log10, x),
                x.log10(),
                1e-12,
                1e-11,
            );
        }
        assert_eq!(run_unary(Which::Log10, 0.0), f64::NEG_INFINITY);
        assert!(run_unary(Which::Log10, -1.0).is_nan());
    }

    // ── pow ─────────────────────────────────────────────────────────────────

    #[test]
    fn pow_matches_f64() {
        // y == 0 and x == 1 short-circuits.
        assert_eq!(run_pow(123.4, 0.0), 1.0);
        assert_eq!(run_pow(1.0, 567.8), 1.0);
        // Positive-base grid (the supported regime), integer and fractional y.
        for i in 0..40 {
            for j in 0..40 {
                let x = 0.01 + 100.0 * (i as f64) / 40.0;
                let y = -5.0 + 10.0 * (j as f64) / 40.0;
                let want = x.powf(y);
                if want.is_finite() {
                    assert_close("pow", x, run_pow(x, y), want, 1e-9, 1e-9);
                }
            }
        }
        // Known limitation: a negative base diverges (ln of negative is NaN).
        assert!(run_pow(-2.0, 2.0).is_nan());
    }
}
