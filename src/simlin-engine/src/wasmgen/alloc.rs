// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: each emitter builds the body of one self-contained wasm
// helper function mirroring the matching `crate::alloc` function. No I/O; the
// only side effect is in `#[cfg(test)]` (which lives in `lower_tests.rs`
// alongside the rest of the lowering harness).

//! Lowering of the bytecode VM's market-clearing allocators
//! (`AllocateAvailable`/`AllocateByPriority`) to WebAssembly (Phase 6).
//!
//! These opcodes route through four self-contained wasm helper functions that
//! port `crate::alloc` *bit-faithfully* -- exact constants, exact Horner
//! evaluation order, exact branch structure, and the exact bisection loop +
//! relative-convergence break -- so the emitted module takes the same numerical
//! path the VM does:
//!
//! - [`emit_erfc_approx`] -- `crate::alloc::erfc_approx` (Abramowitz-Stegun
//!   26.2.17), `call`ing the Phase-2 `exp` helper for the `(-z*z).exp()` factor.
//! - [`emit_normal_cdf`] -- `crate::alloc::normal_cdf`
//!   (`0.5 * erfc_approx(-x / SQRT_2)`).
//! - [`emit_alloc_curve`] -- `crate::alloc::alloc_curve` (all six `ptype % 10`
//!   curve branches + the `ptype >= 10` floor flag).
//! - [`emit_allocate_available`] -- `crate::alloc::allocate_available` (the
//!   `total_demand` short-circuits, the per-type search-range computation, the
//!   100-iteration bisection, and the final per-requester `alloc_curve`).
//!
//! ## Runtime loop vs unrolled
//!
//! [`emit_allocate_available`] is a **runtime-loop** helper: `n` (the requester
//! count) is a runtime value, so it iterates over scratch-memory arrays
//! (`requests`/`profiles`/`out`) with wasm `loop`/`br_if`, never unrolled. The
//! other three helpers are straight-line numeric kernels. The lowering arm
//! (`super::lower`) gathers the request + profile values from the compile-time
//! view stack into the scratch region (an unrolled per-element copy charged
//! against the unroll budget) before `call`ing this helper.
//!
//! ## Why bit-faithful (rather than "close enough")
//!
//! The allocation curves and the bisection are sensitive: `alloc_curve` selects
//! among six analytic survival functions by an integer `ptype % 10`, and the
//! bisection's `total < avail` comparison decides which half to keep at each of
//! 100 steps. Reproducing the Rust reference's exact arithmetic (including the
//! `(-z) * z` / `(-z).exp()` unary-negation order and the `q.is_infinite()`
//! CES guard) keeps the converged price -- and therefore every per-requester
//! allocation -- identical to the VM up to the leaf `exp`/`pow` helpers' own
//! documented tolerance.

use wasm_encoder::{BlockType, Function, Instruction as Ins, ValType};

use super::WasmGenError;
use super::lower::{
    EmitCtx, SLOT_SIZE, emit_fill_temp_nan, emit_view_element_load, f64_const, memarg,
    temp_element_byte_addr,
};
use super::math::emit_horner;
use super::views::ViewDesc;

// ── erfc_approx (alloc.rs:8-21) ──────────────────────────────────────────────

// Abramowitz & Stegun 26.2.17 constants (alloc.rs:12-17). Low-order-first for
// the shared `emit_horner`, whose `acc = acc*t + c` fold reproduces the Rust
// expression `(((((a5*t + a4)*t) + a3)*t + a2)*t + a1)` op-for-op.
const A1: f64 = 0.254829592;
const A2: f64 = -0.284496736;
const A3: f64 = 1.421413741;
const A4: f64 = -1.453152027;
const A5: f64 = 1.061405429;
const AS_P: f64 = 0.3275911;

// `erfc_approx` local layout. Param 0 is `z`; `T` is the reduced argument
// `t = 1/(1 + p*z)`, materialized in a local so `emit_horner` can read it once
// per polynomial term.
const ERFC_Z: u32 = 0;
const ERFC_T: u32 = 1;

/// Emit `erfc_approx(z: f64) -> f64`, porting `crate::alloc::erfc_approx`
/// (Abramowitz-Stegun 26.2.17) bit-faithfully.
///
/// For `z < 0` returns `2.0 - erfc_approx(-z)` (the symmetry the Rust reference
/// uses); else `t = 1/(1 + p*z)` and the result is the degree-5 polynomial
/// `(((((a5*t + a4)*t) + a3)*t + a2)*t + a1) * t * (-z*z).exp()`. The polynomial
/// is evaluated by the shared [`emit_horner`] (identical fold order); `(-z) * z`
/// reproduces Rust's unary-negation precedence (`-z * z == (-z) * z`); the
/// `.exp()` is the Phase-2 `exp` helper (`exp_idx`). The `z < 0` symmetry branch
/// is open-coded as `2 - kernel(-z)` (the kernel is the shared non-negative path),
/// so no self-`call` -- and therefore no forward index to itself -- is needed.
pub(crate) fn emit_erfc_approx(exp_idx: u32) -> Function {
    // One f64 scratch local (ERFC_T) after the `z` param.
    let mut f = Function::new([(1, ValType::F64)]);
    emit_erfc_body(&mut f, exp_idx);
    f.instruction(&Ins::End);
    f
}

/// The body of `erfc_approx` (no terminating `End`). The `z < 0` symmetry branch
/// is open-coded as `2 - erfc_approx_of(-z)` rather than a self-`call`, so the
/// helper needs no forward index to itself: `erfc_approx_of` shares the
/// non-negative-argument kernel.
fn emit_erfc_body(f: &mut Function, exp_idx: u32) {
    // if z < 0 { 2.0 - kernel(-z) } else { kernel(z) }.
    f.instruction(&Ins::LocalGet(ERFC_Z));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    // 2.0 - kernel(-z): negate z in place, run the kernel, subtract from 2.
    f.instruction(&f64_const(2.0));
    f.instruction(&Ins::LocalGet(ERFC_Z));
    f.instruction(&Ins::F64Neg);
    f.instruction(&Ins::LocalSet(ERFC_Z));
    emit_erfc_kernel(f, exp_idx);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::Else);
    emit_erfc_kernel(f, exp_idx);
    f.instruction(&Ins::End);
}

/// The non-negative-argument kernel of `erfc_approx`, leaving the f64 result on
/// the stack: `t = 1/(1 + p*z)`, then `poly(t) * t * (-z*z).exp()`. Reads `z`
/// from [`ERFC_Z`] (already non-negative at every call site).
fn emit_erfc_kernel(f: &mut Function, exp_idx: u32) {
    // t = 1.0 / (1.0 + p * z)
    f.instruction(&f64_const(1.0));
    f.instruction(&f64_const(AS_P));
    f.instruction(&Ins::LocalGet(ERFC_Z));
    f.instruction(&Ins::F64Mul);
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(ERFC_T));

    // poly(t) = (((((a5*t + a4)*t) + a3)*t + a2)*t + a1) -- the shared Horner
    // fold matches this op order exactly.
    emit_horner(f, ERFC_T, &[A1, A2, A3, A4, A5]);
    // * t
    f.instruction(&Ins::LocalGet(ERFC_T));
    f.instruction(&Ins::F64Mul);
    // * (-z * z).exp(): (-z) then * z (Rust unary-neg precedence), then exp().
    f.instruction(&Ins::LocalGet(ERFC_Z));
    f.instruction(&Ins::F64Neg);
    f.instruction(&Ins::LocalGet(ERFC_Z));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::Call(exp_idx));
    f.instruction(&Ins::F64Mul);
}

// ── normal_cdf (alloc.rs:25-30) ──────────────────────────────────────────────

const NCDF_X: u32 = 0;

/// Emit `normal_cdf(x: f64) -> f64`, porting `crate::alloc::normal_cdf`:
/// `if x.is_nan() { NaN } else { 0.5 * erfc_approx(-x / SQRT_2) }`. `erfc_idx`
/// is [`emit_erfc_approx`]'s assigned function index.
pub(crate) fn emit_normal_cdf(erfc_idx: u32) -> Function {
    let mut f = Function::new([]);

    // NaN guard: x != x -> return NaN.
    f.instruction(&Ins::LocalGet(NCDF_X));
    f.instruction(&Ins::LocalGet(NCDF_X));
    f.instruction(&Ins::F64Ne);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // 0.5 * erfc_approx(-x / SQRT_2)
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::LocalGet(NCDF_X));
    f.instruction(&Ins::F64Neg);
    f.instruction(&f64_const(std::f64::consts::SQRT_2));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::Call(erfc_idx));
    f.instruction(&Ins::F64Mul);

    f.instruction(&Ins::End);
    f
}

// ── alloc_curve (alloc.rs:40-129) ────────────────────────────────────────────

// `alloc_curve` param layout (mirrors the Rust signature order).
const CURVE_P: u32 = 0;
const CURVE_REQUEST: u32 = 1;
const CURVE_PTYPE: u32 = 2;
const CURVE_PPRIORITY: u32 = 3;
const CURVE_PWIDTH: u32 = 4;
const CURVE_PEXTRA: u32 = 5;
// Scratch locals (after the six params).
const CURVE_PT_MOD: u32 = 6; // i32 `ptype % 10`
const CURVE_FRACTION: u32 = 7; // f64 the survival fraction
const CURVE_T: u32 = 8; // f64 the rectangular/triangular interpolation `t`
const CURVE_Z: u32 = 9; // f64 the exponential branch `z`
const CURVE_Q: u32 = 10; // f64 the CES branch `q`

/// Emit `alloc_curve(p, request, ptype, ppriority, pwidth, pextra) -> f64`,
/// porting `crate::alloc::alloc_curve` bit-faithfully.
///
/// `request <= 0` returns 0 immediately. Otherwise the survival `fraction` is
/// selected by `ptype % 10` across all six branches (0 fixed, 1 rectangular,
/// 2 triangular, 3 normal via [`normal_cdf`](emit_normal_cdf), 4 exponential
/// via the `exp` helper, 5 CES via the `pow` helper, `_` fixed), then
/// `alloc = request * fraction` is floored when `ptype >= 10`. `ptype` is
/// carried as an f64 (the VM stores profile fields as f64 and casts `pt as i32`);
/// `ptype % 10` and the `ptype >= 10` test reproduce that i32 cast via
/// `i32.trunc_sat_f64_s`. `normal_cdf_idx`/`exp_idx`/`pow_idx` are the helpers'
/// assigned function indices.
pub(crate) fn emit_alloc_curve(normal_cdf_idx: u32, exp_idx: u32, pow_idx: u32) -> Function {
    // Scratch: one i32 (CURVE_PT_MOD) + four f64 (FRACTION/T/Z/Q).
    let mut f = Function::new([(1, ValType::I32), (4, ValType::F64)]);

    // if request <= 0.0 { return 0.0 }
    f.instruction(&Ins::LocalGet(CURVE_REQUEST));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // pt_mod = (ptype as i32) % 10  (truncated remainder, sign of the dividend --
    // wasm `i32.rem_s` matches Rust `%`).
    f.instruction(&Ins::LocalGet(CURVE_PTYPE));
    f.instruction(&Ins::I32TruncSatF64S);
    f.instruction(&Ins::I32Const(10));
    f.instruction(&Ins::I32RemS);
    f.instruction(&Ins::LocalSet(CURVE_PT_MOD));

    // fraction = match pt_mod { 0|_ => fixed, 1 => rect, 2 => tri, 3 => normal,
    //                           4 => exp, 5 => ces }. Emitted as an if/else
    // chain on pt_mod; each arm leaves the fraction on the stack, stored into
    // CURVE_FRACTION below.
    emit_curve_fraction(&mut f, normal_cdf_idx, exp_idx, pow_idx);
    f.instruction(&Ins::LocalSet(CURVE_FRACTION));

    // alloc = request * fraction, parked in CURVE_T (free here) so the floor
    // branch can read it inside both `if` arms (a wasm `If(Result(F64))` does
    // NOT carry the pre-`if` stack value into the block).
    f.instruction(&Ins::LocalGet(CURVE_REQUEST));
    f.instruction(&Ins::LocalGet(CURVE_FRACTION));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(CURVE_T));

    // if ptype >= 10 { alloc.floor() } else { alloc }. `ptype >= 10` tests the
    // original f64 ptype (Rust `ptype >= 10`, an i32 compare; ptype is
    // integer-valued here).
    f.instruction(&Ins::LocalGet(CURVE_PTYPE));
    f.instruction(&Ins::I32TruncSatF64S);
    f.instruction(&Ins::I32Const(10));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&Ins::LocalGet(CURVE_T));
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::Else);
    f.instruction(&Ins::LocalGet(CURVE_T));
    f.instruction(&Ins::End);

    f.instruction(&Ins::End);
    f
}

/// Push the survival `fraction` for the `pt_mod` already in [`CURVE_PT_MOD`],
/// dispatching the six `ptype % 10` branches as a nested if/else chain (each arm
/// a `Result(F64)` leaving exactly one f64). The `_` default and branch `0` are
/// the identical "fixed" survival, so the chain falls through to it.
fn emit_curve_fraction(f: &mut Function, normal_cdf_idx: u32, exp_idx: u32, pow_idx: u32) {
    // if pt_mod == 1 { rect } else if pt_mod == 2 { tri } else if pt_mod == 3
    // { normal } else if pt_mod == 4 { exp } else if pt_mod == 5 { ces }
    // else { fixed }.
    emit_pt_eq(f, 1);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    emit_curve_rectangular(f);
    f.instruction(&Ins::Else);

    emit_pt_eq(f, 2);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    emit_curve_triangular(f);
    f.instruction(&Ins::Else);

    emit_pt_eq(f, 3);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    emit_curve_normal(f, normal_cdf_idx);
    f.instruction(&Ins::Else);

    emit_pt_eq(f, 4);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    emit_curve_exponential(f, exp_idx);
    f.instruction(&Ins::Else);

    emit_pt_eq(f, 5);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    emit_curve_ces(f, pow_idx);
    f.instruction(&Ins::Else);

    // Default (pt_mod == 0 or anything else): the fixed survival.
    emit_curve_fixed(f);

    f.instruction(&Ins::End); // 5
    f.instruction(&Ins::End); // 4
    f.instruction(&Ins::End); // 3
    f.instruction(&Ins::End); // 2
    f.instruction(&Ins::End); // 1
}

/// Push the i32 condition `pt_mod == n`.
fn emit_pt_eq(f: &mut Function, n: i32) {
    f.instruction(&Ins::LocalGet(CURVE_PT_MOD));
    f.instruction(&Ins::I32Const(n));
    f.instruction(&Ins::I32Eq);
}

/// Branch 0 / `_`: fixed quantity -- `if p <= ppriority { 1.0 } else { 0.0 }`.
fn emit_curve_fixed(f: &mut Function) {
    f.instruction(&f64_const(1.0));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalGet(CURVE_P));
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&Ins::F64Le); // p <= ppriority
    f.instruction(&Ins::Select); // 1.0 if p<=ppriority else 0.0
}

/// Branch 1: rectangular survival. `lo = ppriority - pwidth; hi = ppriority +
/// pwidth; if p <= lo { 1 } else if p >= hi { 0 } else { (hi - p)/(hi - lo) }`.
/// `lo`/`hi` are recomputed inline at each use (matching the Rust let-bindings'
/// values; the FP result is identical) to avoid extra scratch locals.
fn emit_curve_rectangular(f: &mut Function) {
    // if p <= lo { 1.0 }
    f.instruction(&Ins::LocalGet(CURVE_P));
    emit_lo(f);
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::Else);
    // else if p >= hi { 0.0 } else { (hi - p) / (hi - lo) }
    f.instruction(&Ins::LocalGet(CURVE_P));
    emit_hi(f);
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Else);
    emit_hi_minus_p_over_hi_minus_lo(f);
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);
}

/// Branch 2: triangular survival. `lo`/`hi` as in rectangular; `if p <= lo { 1 }
/// else if p >= hi { 0 } else if p <= ppriority { t = (hi-p)/(hi-lo); 1 -
/// 2(1-t)^2 } else { t = (hi-p)/(hi-lo); 2 t^2 }`.
fn emit_curve_triangular(f: &mut Function) {
    // if p <= lo { 1.0 }
    f.instruction(&Ins::LocalGet(CURVE_P));
    emit_lo(f);
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::Else);
    // else if p >= hi { 0.0 }
    f.instruction(&Ins::LocalGet(CURVE_P));
    emit_hi(f);
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Else);
    // t = (hi - p) / (hi - lo)
    emit_hi_minus_p_over_hi_minus_lo(f);
    f.instruction(&Ins::LocalSet(CURVE_T));
    // else if p <= ppriority { 1 - 2*(1-t)*(1-t) } else { 2*t*t }
    f.instruction(&Ins::LocalGet(CURVE_P));
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&Ins::F64Le); // p <= ppriority
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    // 1.0 - 2.0 * (1.0 - t) * (1.0 - t)
    f.instruction(&f64_const(1.0));
    f.instruction(&f64_const(2.0));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(CURVE_T));
    f.instruction(&Ins::F64Sub); // (1 - t)
    f.instruction(&Ins::F64Mul); // 2 * (1 - t)
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(CURVE_T));
    f.instruction(&Ins::F64Sub); // (1 - t)
    f.instruction(&Ins::F64Mul); // 2 * (1 - t) * (1 - t)
    f.instruction(&Ins::F64Sub); // 1 - 2*(1-t)*(1-t)
    f.instruction(&Ins::Else);
    // 2.0 * t * t
    f.instruction(&f64_const(2.0));
    f.instruction(&Ins::LocalGet(CURVE_T));
    f.instruction(&Ins::F64Mul); // 2 * t
    f.instruction(&Ins::LocalGet(CURVE_T));
    f.instruction(&Ins::F64Mul); // 2 * t * t
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);
}

/// Branch 3: normal survival. `if pwidth <= 0 { if p <= ppriority { 1 } else
/// { 0 } } else { normal_cdf((ppriority - p) / pwidth) }`.
fn emit_curve_normal(f: &mut Function, normal_cdf_idx: u32) {
    f.instruction(&Ins::LocalGet(CURVE_PWIDTH));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le); // pwidth <= 0
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    emit_curve_fixed(f);
    f.instruction(&Ins::Else);
    // normal_cdf((ppriority - p) / pwidth)
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&Ins::LocalGet(CURVE_P));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalGet(CURVE_PWIDTH));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::Call(normal_cdf_idx));
    f.instruction(&Ins::End);
}

/// Branch 4: symmetric exponential survival. `if pwidth <= 0 { fixed } else
/// { z = (p - ppriority) / pwidth; if z > 0 { 0.5 * (-z).exp() } else { 1 - 0.5
/// * z.exp() } }`.
fn emit_curve_exponential(f: &mut Function, exp_idx: u32) {
    f.instruction(&Ins::LocalGet(CURVE_PWIDTH));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le); // pwidth <= 0
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    emit_curve_fixed(f);
    f.instruction(&Ins::Else);
    // z = (p - ppriority) / pwidth
    f.instruction(&Ins::LocalGet(CURVE_P));
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalGet(CURVE_PWIDTH));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(CURVE_Z));
    // if z > 0 { 0.5 * (-z).exp() } else { 1.0 - 0.5 * z.exp() }
    f.instruction(&Ins::LocalGet(CURVE_Z));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    // 0.5 * (-z).exp()
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::LocalGet(CURVE_Z));
    f.instruction(&Ins::F64Neg);
    f.instruction(&Ins::Call(exp_idx));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::Else);
    // 1.0 - 0.5 * z.exp()
    f.instruction(&f64_const(1.0));
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::LocalGet(CURVE_Z));
    f.instruction(&Ins::Call(exp_idx));
    f.instruction(&Ins::F64Mul); // 0.5 * z.exp()
    f.instruction(&Ins::F64Sub); // 1 - 0.5 * z.exp()
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);
}

/// Branch 5: constant elasticity of substitution (CES). `if p <= 0 { 1 } else
/// if ppriority <= 0 { 0 } else { ratio = ppriority / p; q = ratio.powf(pextra);
/// if q.is_infinite() { 1 } else { q / (1 + q) } }`.
fn emit_curve_ces(f: &mut Function, pow_idx: u32) {
    // if p <= 0 { 1.0 }
    f.instruction(&Ins::LocalGet(CURVE_P));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::Else);
    // else if ppriority <= 0 { 0.0 }
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Else);
    // q = (ppriority / p).powf(pextra)
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&Ins::LocalGet(CURVE_P));
    f.instruction(&Ins::F64Div); // ratio
    f.instruction(&Ins::LocalGet(CURVE_PEXTRA));
    f.instruction(&Ins::Call(pow_idx));
    f.instruction(&Ins::LocalSet(CURVE_Q));
    // if q.is_infinite() { 1.0 } else { q / (1.0 + q) }
    f.instruction(&Ins::LocalGet(CURVE_Q));
    f.instruction(&Ins::F64Abs);
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::F64Eq); // |q| == inf  (q.is_infinite())
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::Else);
    // q / (1.0 + q)
    f.instruction(&Ins::LocalGet(CURVE_Q));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(CURVE_Q));
    f.instruction(&Ins::F64Add); // 1 + q
    f.instruction(&Ins::F64Div); // q / (1 + q)
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);
}

/// Push `ppriority - pwidth` (the rectangular/triangular `lo`).
fn emit_lo(f: &mut Function) {
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&Ins::LocalGet(CURVE_PWIDTH));
    f.instruction(&Ins::F64Sub);
}

/// Push `ppriority + pwidth` (the rectangular/triangular `hi`).
fn emit_hi(f: &mut Function) {
    f.instruction(&Ins::LocalGet(CURVE_PPRIORITY));
    f.instruction(&Ins::LocalGet(CURVE_PWIDTH));
    f.instruction(&Ins::F64Add);
}

/// Push `(hi - p) / (hi - lo)` where `lo = ppriority - pwidth`, `hi = ppriority
/// + pwidth`. `hi - lo == 2*pwidth`, but the Rust reference computes `(hi - lo)`
/// from the let-bound `hi`/`lo`, so reproduce that exact subtraction.
fn emit_hi_minus_p_over_hi_minus_lo(f: &mut Function) {
    // hi - p
    emit_hi(f);
    f.instruction(&Ins::LocalGet(CURVE_P));
    f.instruction(&Ins::F64Sub);
    // hi - lo
    emit_hi(f);
    emit_lo(f);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Div);
}

// ── allocate_available (alloc.rs:136-199) ────────────────────────────────────

// `allocate_available(requests_ptr: i32, n: i32, profiles_ptr: i32, avail: f64,
// out_ptr: i32) -> ()` local layout. `requests_ptr`/`profiles_ptr`/`out_ptr` are
// byte addresses into the scratch region; `profiles` is 4 f64/requester laid out
// `(ptype, ppriority, pwidth, pextra)`.
const ALLOC_REQ_PTR: u32 = 0;
const ALLOC_N: u32 = 1;
const ALLOC_PROF_PTR: u32 = 2;
const ALLOC_AVAIL: u32 = 3;
const ALLOC_OUT_PTR: u32 = 4;
// Scratch locals (after the five params).
const ALLOC_I: u32 = 5; // i32 loop index
const ALLOC_TOTAL_DEMAND: u32 = 6; // f64 Σ requests where r > 0
const ALLOC_R: u32 = 7; // f64 a request value
const ALLOC_P_MIN: u32 = 8; // f64 search-range lower bound
const ALLOC_P_MAX: u32 = 9; // f64 search-range upper bound
const ALLOC_SPREAD: u32 = 10; // f64 per-profile spread
const ALLOC_PPRIORITY: u32 = 11; // f64 a profile's ppriority
const ALLOC_PWIDTH: u32 = 12; // f64 a profile's pwidth
const ALLOC_PT_MOD: u32 = 13; // i32 a profile's ptype % 10
const ALLOC_LO: u32 = 14; // f64 bisection low
const ALLOC_HI: u32 = 15; // f64 bisection high
const ALLOC_MID: u32 = 16; // f64 bisection midpoint
const ALLOC_TOTAL: u32 = 17; // f64 Σ alloc_curve(mid, ...)
const ALLOC_ITER: u32 = 18; // i32 bisection iteration counter
const ALLOC_PSTAR: u32 = 19; // f64 the converged price

// Bytes per profile tuple (4 f64) and per request/out slot (1 f64).
const PROFILE_BYTES: i32 = 32;
const SLOT_BYTES: i32 = 8;

/// Emit `allocate_available(requests_ptr, n, profiles_ptr, avail, out_ptr)`,
/// porting `crate::alloc::allocate_available` bit-faithfully over scratch-memory
/// arrays.
///
/// The three short-circuits (`n == 0` -> nothing written; `avail >=
/// total_demand` -> each requester gets `r.max(0)`; `avail <= 0` -> zeros)
/// mirror the Rust early returns. Otherwise the per-type search range
/// `[p_min, p_max]` is computed from the profiles' `spread`, then a 100-iteration
/// bisection finds the market-clearing price (the `total < avail` -> `hi = mid`
/// step and the `|hi - lo| < 1e-14 * (1 + |hi|)` relative-convergence break),
/// and `out[i] = alloc_curve(p_star, requests[i], ...)` is written for every
/// requester. A runtime loop (never unrolled): `n` is a runtime value.
/// `alloc_curve_idx` is [`emit_alloc_curve`]'s assigned function index.
pub(crate) fn emit_allocate_available(alloc_curve_idx: u32) -> Function {
    // Scratch: i32 (I), f64 (TOTAL_DEMAND, R, P_MIN, P_MAX, SPREAD, PPRIORITY,
    // PWIDTH), i32 (PT_MOD), f64 (LO, HI, MID, TOTAL), i32 (ITER), f64 (PSTAR).
    // Declaration order fixes the indices ALLOC_I..ALLOC_PSTAR.
    let mut f = Function::new([
        (1, ValType::I32),
        (7, ValType::F64),
        (1, ValType::I32),
        (4, ValType::F64),
        (1, ValType::I32),
        (1, ValType::F64),
    ]);

    // if n == 0 { return }  (the Rust `if n == 0 { return vec![] }`).
    f.instruction(&Ins::LocalGet(ALLOC_N));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // total_demand = Σ requests[i] where requests[i] > 0.0.
    emit_total_demand(&mut f);

    // if avail >= total_demand { out[i] = requests[i].max(0.0); return }
    f.instruction(&Ins::LocalGet(ALLOC_AVAIL));
    f.instruction(&Ins::LocalGet(ALLOC_TOTAL_DEMAND));
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::If(BlockType::Empty));
    emit_full_grant(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // if avail <= 0.0 { out[i] = 0.0; return }
    f.instruction(&Ins::LocalGet(ALLOC_AVAIL));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Empty));
    emit_zero_out(&mut f);
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    // Compute the search range [p_min, p_max] from the profiles.
    emit_search_range(&mut f);

    // 100-iteration bisection for the market-clearing price.
    emit_bisection(&mut f, alloc_curve_idx);

    // p_star = (lo + hi) / 2.0; out[i] = alloc_curve(p_star, requests[i], ...).
    f.instruction(&Ins::LocalGet(ALLOC_LO));
    f.instruction(&Ins::LocalGet(ALLOC_HI));
    f.instruction(&Ins::F64Add);
    f.instruction(&f64_const(2.0));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(ALLOC_PSTAR));
    emit_final_allocations(&mut f, alloc_curve_idx);

    f.instruction(&Ins::End);
    f
}

/// `total_demand = Σ requests[i] where requests[i] > 0.0` into
/// [`ALLOC_TOTAL_DEMAND`]. A runtime `for i in 0..n` loop.
fn emit_total_demand(f: &mut Function) {
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(ALLOC_TOTAL_DEMAND));
    emit_for_n(f, |f| {
        // r = requests[i]
        emit_load_request(f);
        f.instruction(&Ins::LocalSet(ALLOC_R));
        // if r > 0.0 { total_demand += r }
        f.instruction(&Ins::LocalGet(ALLOC_R));
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::F64Gt);
        f.instruction(&Ins::If(BlockType::Empty));
        f.instruction(&Ins::LocalGet(ALLOC_TOTAL_DEMAND));
        f.instruction(&Ins::LocalGet(ALLOC_R));
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::LocalSet(ALLOC_TOTAL_DEMAND));
        f.instruction(&Ins::End);
    });
}

/// The `avail >= total_demand` arm: `out[i] = requests[i].max(0.0)` for every
/// requester. `f64::max` is NaN-ignoring; reproduce it with the compare-select
/// form (`r > 0 ? r : 0` is `r.max(0.0)` for a non-NaN `r`, and a NaN request
/// would be ignored by `f64::max` -- but the Rust path stores `r.max(0.0)` which
/// is `0.0` for a NaN `r`, matched here since `NaN > 0.0` is false).
fn emit_full_grant(f: &mut Function) {
    emit_for_n(f, |f| {
        // out[i] = max(requests[i], 0.0)
        emit_out_addr(f);
        // value = r > 0.0 ? r : 0.0  (== f64::max(r, 0.0) for non-NaN; for NaN r
        // this yields 0.0, matching Rust `NaN.max(0.0) == 0.0`).
        emit_load_request(f);
        f.instruction(&Ins::LocalSet(ALLOC_R));
        f.instruction(&Ins::LocalGet(ALLOC_R));
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::LocalGet(ALLOC_R));
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::F64Gt); // r > 0.0
        f.instruction(&Ins::Select); // r if r>0 else 0.0
        f.instruction(&Ins::F64Store(f64_memarg()));
    });
}

/// The `avail <= 0.0` arm: `out[i] = 0.0` for every requester.
fn emit_zero_out(f: &mut Function) {
    emit_for_n(f, |f| {
        emit_out_addr(f);
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::F64Store(f64_memarg()));
    });
}

/// Compute `[p_min, p_max]` from the profiles (alloc.rs:154-169): `p_min =
/// INFINITY`, `p_max = NEG_INFINITY`; for each profile `spread = match ptype % 10
/// { 0 => 1, 1|2 => pwidth, 3 => pwidth*6, 4 => pwidth*10, 5 => ppriority*10,
/// _ => 1 }`, then `p_min = min(p_min, ppriority - spread)`, `p_max =
/// max(p_max, ppriority + spread)`. `f64::min`/`f64::max` are NaN-ignoring;
/// realistic profiles never carry NaN, and the reference uses them, so the
/// NaN-ignoring compare-select form is reproduced for fidelity.
fn emit_search_range(f: &mut Function) {
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::LocalSet(ALLOC_P_MIN));
    f.instruction(&f64_const(f64::NEG_INFINITY));
    f.instruction(&Ins::LocalSet(ALLOC_P_MAX));

    emit_for_n(f, |f| {
        // ppriority = profiles[i].1; pwidth = profiles[i].2; pt_mod =
        // (profiles[i].0 as i32) % 10.
        emit_load_profile_field(f, 1);
        f.instruction(&Ins::LocalSet(ALLOC_PPRIORITY));
        emit_load_profile_field(f, 2);
        f.instruction(&Ins::LocalSet(ALLOC_PWIDTH));
        emit_load_profile_field(f, 0);
        f.instruction(&Ins::I32TruncSatF64S);
        f.instruction(&Ins::I32Const(10));
        f.instruction(&Ins::I32RemS);
        f.instruction(&Ins::LocalSet(ALLOC_PT_MOD));

        // spread = match pt_mod { 1|2 => pwidth, 3 => pwidth*6, 4 => pwidth*10,
        //                         5 => ppriority*10, 0|_ => 1.0 }.
        emit_spread(f);
        f.instruction(&Ins::LocalSet(ALLOC_SPREAD));

        // p_min = f64::min(p_min, ppriority - spread)
        f.instruction(&Ins::LocalGet(ALLOC_P_MIN));
        f.instruction(&Ins::LocalGet(ALLOC_PPRIORITY));
        f.instruction(&Ins::LocalGet(ALLOC_SPREAD));
        f.instruction(&Ins::F64Sub);
        emit_f64_min(f);
        f.instruction(&Ins::LocalSet(ALLOC_P_MIN));

        // p_max = f64::max(p_max, ppriority + spread)
        f.instruction(&Ins::LocalGet(ALLOC_P_MAX));
        f.instruction(&Ins::LocalGet(ALLOC_PPRIORITY));
        f.instruction(&Ins::LocalGet(ALLOC_SPREAD));
        f.instruction(&Ins::F64Add);
        emit_f64_max(f);
        f.instruction(&Ins::LocalSet(ALLOC_P_MAX));
    });
}

/// Push the per-profile `spread` for the `pt_mod` in [`ALLOC_PT_MOD`] (uses
/// [`ALLOC_PWIDTH`]/[`ALLOC_PPRIORITY`]): 1 (0/_), pwidth (1/2), pwidth*6 (3),
/// pwidth*10 (4), ppriority*10 (5). Emitted as a nested if/else chain.
fn emit_spread(f: &mut Function) {
    // pt_mod == 1 || pt_mod == 2 -> pwidth
    f.instruction(&Ins::LocalGet(ALLOC_PT_MOD));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::LocalGet(ALLOC_PT_MOD));
    f.instruction(&Ins::I32Const(2));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::I32Or);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&Ins::LocalGet(ALLOC_PWIDTH));
    f.instruction(&Ins::Else);

    // pt_mod == 3 -> pwidth * 6.0
    f.instruction(&Ins::LocalGet(ALLOC_PT_MOD));
    f.instruction(&Ins::I32Const(3));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&Ins::LocalGet(ALLOC_PWIDTH));
    f.instruction(&f64_const(6.0));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::Else);

    // pt_mod == 4 -> pwidth * 10.0
    f.instruction(&Ins::LocalGet(ALLOC_PT_MOD));
    f.instruction(&Ins::I32Const(4));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&Ins::LocalGet(ALLOC_PWIDTH));
    f.instruction(&f64_const(10.0));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::Else);

    // pt_mod == 5 -> ppriority * 10.0
    f.instruction(&Ins::LocalGet(ALLOC_PT_MOD));
    f.instruction(&Ins::I32Const(5));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&Ins::LocalGet(ALLOC_PPRIORITY));
    f.instruction(&f64_const(10.0));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::Else);

    // default (pt_mod == 0 or anything else) -> 1.0
    f.instruction(&f64_const(1.0));

    f.instruction(&Ins::End); // 5
    f.instruction(&Ins::End); // 4
    f.instruction(&Ins::End); // 3
    f.instruction(&Ins::End); // 1|2
}

/// The 100-iteration bisection (alloc.rs:171-190): `lo = p_min; hi = p_max; for
/// _ in 0..100 { mid = (lo+hi)/2; total = Σ alloc_curve(mid, ...); if total <
/// avail { hi = mid } else { lo = mid }; if |hi-lo| < 1e-14*(1+|hi|) { break } }`.
fn emit_bisection(f: &mut Function, alloc_curve_idx: u32) {
    // lo = p_min; hi = p_max; iter = 0
    f.instruction(&Ins::LocalGet(ALLOC_P_MIN));
    f.instruction(&Ins::LocalSet(ALLOC_LO));
    f.instruction(&Ins::LocalGet(ALLOC_P_MAX));
    f.instruction(&Ins::LocalSet(ALLOC_HI));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(ALLOC_ITER));

    f.instruction(&Ins::Block(BlockType::Empty)); // $bisect_exit
    f.instruction(&Ins::Loop(BlockType::Empty)); // $bisect

    // while-head: if !(iter < 100) break $bisect_exit  (br depth 1).
    f.instruction(&Ins::LocalGet(ALLOC_ITER));
    f.instruction(&Ins::I32Const(100));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    // mid = (lo + hi) / 2.0
    f.instruction(&Ins::LocalGet(ALLOC_LO));
    f.instruction(&Ins::LocalGet(ALLOC_HI));
    f.instruction(&Ins::F64Add);
    f.instruction(&f64_const(2.0));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(ALLOC_MID));

    // total = Σ_{i<n} alloc_curve(mid, requests[i], profiles[i]...)
    emit_total_at_price(f, ALLOC_MID, alloc_curve_idx);
    f.instruction(&Ins::LocalSet(ALLOC_TOTAL));

    // if total < avail { hi = mid } else { lo = mid }
    f.instruction(&Ins::LocalGet(ALLOC_TOTAL));
    f.instruction(&Ins::LocalGet(ALLOC_AVAIL));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(ALLOC_MID));
    f.instruction(&Ins::LocalSet(ALLOC_HI));
    f.instruction(&Ins::Else);
    f.instruction(&Ins::LocalGet(ALLOC_MID));
    f.instruction(&Ins::LocalSet(ALLOC_LO));
    f.instruction(&Ins::End);

    // if |hi - lo| < 1e-14 * (1.0 + |hi|) { break $bisect_exit }
    f.instruction(&Ins::LocalGet(ALLOC_HI));
    f.instruction(&Ins::LocalGet(ALLOC_LO));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Abs);
    f.instruction(&f64_const(1e-14));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(ALLOC_HI));
    f.instruction(&Ins::F64Abs);
    f.instruction(&Ins::F64Add); // 1 + |hi|
    f.instruction(&Ins::F64Mul); // 1e-14 * (1 + |hi|)
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::BrIf(1)); // break $bisect_exit

    // iter += 1; continue $bisect
    f.instruction(&Ins::LocalGet(ALLOC_ITER));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(ALLOC_ITER));
    f.instruction(&Ins::Br(0));

    f.instruction(&Ins::End); // end $bisect loop
    f.instruction(&Ins::End); // end $bisect_exit block
}

/// Push `Σ_{i<n} alloc_curve(price, requests[i], profiles[i]...)` for the price
/// in `price_local`. A runtime `for i in 0..n` accumulating into a scratch
/// f64 left on the stack at the end.
fn emit_total_at_price(f: &mut Function, price_local: u32, alloc_curve_idx: u32) {
    // ALLOC_SPREAD is the running sum here. It was only live inside
    // `emit_search_range` (which has finished by the time the bisection runs),
    // and `alloc_curve` is a separate function that cannot touch this helper's
    // locals, so reusing it as the fold accumulator is safe.
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(ALLOC_SPREAD));
    emit_for_n(f, |f| {
        // sum += alloc_curve(price, requests[i], ptype, ppriority, pwidth, pextra)
        f.instruction(&Ins::LocalGet(ALLOC_SPREAD));
        emit_alloc_curve_call(f, price_local, alloc_curve_idx);
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::LocalSet(ALLOC_SPREAD));
    });
    f.instruction(&Ins::LocalGet(ALLOC_SPREAD));
}

/// `out[i] = alloc_curve(p_star, requests[i], profiles[i]...)` for every
/// requester (alloc.rs:193-198).
fn emit_final_allocations(f: &mut Function, alloc_curve_idx: u32) {
    emit_for_n(f, |f| {
        emit_out_addr(f);
        emit_alloc_curve_call(f, ALLOC_PSTAR, alloc_curve_idx);
        f.instruction(&Ins::F64Store(f64_memarg()));
    });
}

/// Push `alloc_curve(price, requests[i], profiles[i].0, .1, .2, .3)` -- the six
/// arguments in order, then the `call`. Reads `requests[i]`/`profiles[i]` for the
/// current loop index `i` ([`ALLOC_I`]); `price` is the f64 in `price_local`.
fn emit_alloc_curve_call(f: &mut Function, price_local: u32, alloc_curve_idx: u32) {
    f.instruction(&Ins::LocalGet(price_local)); // p
    emit_load_request(f); // request = requests[i]
    emit_load_profile_field(f, 0); // ptype
    emit_load_profile_field(f, 1); // ppriority
    emit_load_profile_field(f, 2); // pwidth
    emit_load_profile_field(f, 3); // pextra
    f.instruction(&Ins::Call(alloc_curve_idx));
}

/// Emit a runtime `for i in 0..n` loop (`ALLOC_I` is the index), invoking `body`
/// once per iteration. `body` must be operand-stack balanced.
fn emit_for_n(f: &mut Function, body: impl FnOnce(&mut Function)) {
    // i = 0
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(ALLOC_I));

    f.instruction(&Ins::Block(BlockType::Empty)); // $exit
    f.instruction(&Ins::Loop(BlockType::Empty)); // $loop

    // if !(i < n) break $exit  (br depth 1)
    f.instruction(&Ins::LocalGet(ALLOC_I));
    f.instruction(&Ins::LocalGet(ALLOC_N));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    body(f);

    // i += 1; continue $loop
    f.instruction(&Ins::LocalGet(ALLOC_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(ALLOC_I));
    f.instruction(&Ins::Br(0));

    f.instruction(&Ins::End); // end $loop
    f.instruction(&Ins::End); // end $exit
}

/// Push `requests[i]` (the f64 at `requests_ptr + i*8`).
fn emit_load_request(f: &mut Function) {
    f.instruction(&Ins::LocalGet(ALLOC_REQ_PTR));
    f.instruction(&Ins::LocalGet(ALLOC_I));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(f64_memarg()));
}

/// Push the store *address* for `out[i]` (`out_ptr + i*8`), to be followed by the
/// value then an `f64.store` (`f64.store` consumes `[addr_i32, value_f64]`).
fn emit_out_addr(f: &mut Function) {
    f.instruction(&Ins::LocalGet(ALLOC_OUT_PTR));
    f.instruction(&Ins::LocalGet(ALLOC_I));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
}

/// Push `profiles[i].field` (the f64 at `profiles_ptr + i*32 + field*8`), with
/// `field in {0,1,2,3}` for `(ptype, ppriority, pwidth, pextra)`.
fn emit_load_profile_field(f: &mut Function, field: i32) {
    f.instruction(&Ins::LocalGet(ALLOC_PROF_PTR));
    f.instruction(&Ins::LocalGet(ALLOC_I));
    f.instruction(&Ins::I32Const(PROFILE_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    // field offset rides in the memarg.offset (a compile-time constant).
    f.instruction(&Ins::F64Load(f64_memarg_off((field * SLOT_BYTES) as u64)));
}

/// Push `f64::min(a, b)` for `[a, b]` on the stack, reproducing Rust's
/// NaN-ignoring `f64::min` (the search-range profiles never carry NaN in a real
/// model, but the reference uses `f64::min`, so the NaN-ignoring form is kept).
/// Built as nested `select`s; mirrors `super::vector`'s `emit_f64_minmax_rust`,
/// but uses this helper's own scratch locals.
fn emit_f64_min(f: &mut Function) {
    emit_f64_minmax(f, true);
}

/// Push `f64::max(a, b)` for `[a, b]` on the stack (NaN-ignoring).
fn emit_f64_max(f: &mut Function) {
    emit_f64_minmax(f, false);
}

/// Shared body of [`emit_f64_min`]/[`emit_f64_max`]: consume `[a, b]` and push
/// `f64::min(a,b)` (`want_min`) or `f64::max(a,b)`, ignoring a NaN operand (if
/// both NaN, NaN). Parks `a`/`b` in scratch locals reused from the bisection
/// (`ALLOC_LO`/`ALLOC_HI`/`ALLOC_MID` are not yet live when the search range is
/// computed, so they are free here). Three nested `select`s in the wasm
/// "deeper operand wins when cond != 0" form, matching `crate::vm`'s `f64::min`/
/// `max` reductions.
fn emit_f64_minmax(f: &mut Function, want_min: bool) {
    let a = ALLOC_LO;
    let b = ALLOC_HI;
    let r = ALLOC_MID;
    // [a, b] on the stack (b on top); park them.
    f.instruction(&Ins::LocalSet(b));
    f.instruction(&Ins::LocalSet(a));

    // core = (a {<,>} b) ? a : b -> r
    f.instruction(&Ins::LocalGet(a));
    f.instruction(&Ins::LocalGet(b));
    f.instruction(&Ins::LocalGet(a));
    f.instruction(&Ins::LocalGet(b));
    if want_min {
        f.instruction(&Ins::F64Lt);
    } else {
        f.instruction(&Ins::F64Gt);
    }
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalSet(r));

    // r = (b is NaN) ? a : r
    f.instruction(&Ins::LocalGet(a));
    f.instruction(&Ins::LocalGet(r));
    f.instruction(&Ins::LocalGet(b));
    f.instruction(&Ins::LocalGet(b));
    f.instruction(&Ins::F64Ne); // b != b
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalSet(r));

    // result = (a is NaN) ? b : r
    f.instruction(&Ins::LocalGet(b));
    f.instruction(&Ins::LocalGet(r));
    f.instruction(&Ins::LocalGet(a));
    f.instruction(&Ins::LocalGet(a));
    f.instruction(&Ins::F64Ne); // a != a
    f.instruction(&Ins::Select);
}

/// An 8-byte (f64) memory access at offset 0, naturally aligned (the scratch
/// region is 8-byte aligned).
fn f64_memarg() -> wasm_encoder::MemArg {
    f64_memarg_off(0)
}

/// An 8-byte (f64) memory access at a static byte `offset`.
fn f64_memarg_off(offset: u64) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset,
        align: 3, // log2(8): an 8-byte f64 access
        memory_index: 0,
    }
}

// ── opcode lowering arms (vm.rs:2631-2794) ───────────────────────────────────

/// Lower `AllocateAvailable { write_temp_id }`, mirroring `vm.rs:2631-2721`. The
/// views are `profile_view = top`, `requests_view = top-1`; `avail` is the f64
/// on top of the wasm operand stack (the VM pops it). Gathers the `n =
/// requests_view.size()` request values + the per-requester profile tuples into
/// the allocation scratch region, `call`s the [`emit_allocate_available`] helper,
/// then copies the `n` results into temp `write_temp_id`. An invalid input view
/// fills the whole destination temp region with NaN.
///
/// `pp_cols` reproduces the VM's `if !pp_values.is_empty() && n>0 &&
/// pp_size%n==0 { pp_size/n } else { 4 }`, and each profile field
/// `(ptype, ppriority, pwidth, pextra)` is read from `pp_values[i*pp_cols + j]`
/// with the VM's defaults `(0.0, 0.0, 1.0, 0.0)` when the index is out of range
/// -- all resolved at compile time (the view sizes and indices are static).
pub(crate) fn emit_allocate_available_op(
    requests_view: &ViewDesc,
    profile_view: &ViewDesc,
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    // A dynamically-subscripted view is fine: the per-element gather routes
    // through `emit_view_element_load`, which folds the view's runtime offset
    // addend and per-element validity guard, and the op-level gate below takes
    // the VM's whole-op `!is_valid -> fill_temp_nan` short-circuit.

    // Pop `avail` (top) into the scratch f64 before the gate, so both gate arms
    // are operand-balanced.
    let avail = ctx.scratch_local;
    f.instruction(&Ins::LocalSet(avail));

    let n = requests_view.size();
    let pp_size = profile_view.size();
    // pp_cols: pp_size/n when the flattened profile array divides evenly into n
    // requesters, else 4 (vm.rs:2680-2685).
    let pp_cols = if pp_size > 0 && n > 0 && pp_size.is_multiple_of(n) {
        pp_size / n
    } else {
        4
    };

    emit_with_validity_gate(
        &[requests_view, profile_view],
        write_temp_id,
        ctx,
        f,
        |ctx, f| {
            // Gather requests[i] -> scratch req region.
            let (req_base, prof_base, out_base) = alloc_scratch_layout(ctx, n);
            for i in 0..n {
                f.instruction(&Ins::I32Const(0));
                emit_view_element_load(requests_view, i, ctx, f)?;
                f.instruction(&Ins::F64Store(memarg(
                    req_base + (i as u64) * u64::from(SLOT_SIZE),
                )));
            }

            // Build per-requester profile tuples (ptype, ppriority, pwidth, pextra)
            // from pp_values[i*pp_cols + j], defaulting (0,0,1,0) out of range.
            const DEFAULTS: [f64; 4] = [0.0, 0.0, 1.0, 0.0];
            for i in 0..n {
                for (j, &default) in DEFAULTS.iter().enumerate() {
                    let prof_addr =
                        prof_base + (i as u64) * (PROFILE_BYTES as u64) + (j as u64) * 8;
                    f.instruction(&Ins::I32Const(0));
                    let flat = i * pp_cols + j;
                    if flat < pp_size {
                        emit_view_element_load(profile_view, flat, ctx, f)?;
                    } else {
                        f.instruction(&f64_const(default));
                    }
                    f.instruction(&Ins::F64Store(memarg(prof_addr)));
                }
            }

            // allocate_available(req_base, n, prof_base, avail, out_base)
            f.instruction(&Ins::I32Const(req_base as i32));
            f.instruction(&Ins::I32Const(n as i32));
            f.instruction(&Ins::I32Const(prof_base as i32));
            f.instruction(&Ins::LocalGet(avail));
            f.instruction(&Ins::I32Const(out_base as i32));
            f.instruction(&Ins::Call(ctx.helpers.allocate_available));

            // Copy out[i] -> temp[write_temp_id][i].
            emit_copy_out_to_temp(out_base, n, write_temp_id, ctx, f)
        },
    )
}

/// Lower `AllocateByPriority { write_temp_id }`, mirroring `vm.rs:2723-2794`. The
/// views are `priority_view = top`, `requests_view = top-1`; the operand stack
/// holds `supply` on top and `width` beneath (the VM pops `supply` then
/// `width`). Gathers requests, synthesizes rectangular profiles `(1.0,
/// priorities[i] or 0.0, width, 0.0)`, `call`s [`emit_allocate_available`] with
/// `supply` as the available amount, then copies results into the temp.
pub(crate) fn emit_allocate_by_priority_op(
    requests_view: &ViewDesc,
    priority_view: &ViewDesc,
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    // A dynamically-subscripted view is handled by `emit_view_element_load`
    // (runtime offset + per-element validity) and the op-level gate below; see
    // `emit_allocate_available_op`.

    // Pop `supply` (top) then `width` into scratch f64s, before the gate.
    let supply = ctx.scratch_local;
    let width = ctx.vector_f64_locals[0];
    f.instruction(&Ins::LocalSet(supply));
    f.instruction(&Ins::LocalSet(width));

    let n = requests_view.size();
    let pri_size = priority_view.size();

    emit_with_validity_gate(
        &[requests_view, priority_view],
        write_temp_id,
        ctx,
        f,
        |ctx, f| {
            let (req_base, prof_base, out_base) = alloc_scratch_layout(ctx, n);
            // Gather requests[i].
            for i in 0..n {
                f.instruction(&Ins::I32Const(0));
                emit_view_element_load(requests_view, i, ctx, f)?;
                f.instruction(&Ins::F64Store(memarg(
                    req_base + (i as u64) * u64::from(SLOT_SIZE),
                )));
            }

            // Rectangular profiles: (ptype=1, ppriority=priorities[i] or 0, pwidth=
            // width, pextra=0). Fields 0/3 are the constants 1.0/0.0; field 1 is the
            // priority view element (default 0.0 out of range); field 2 is the
            // runtime `width` local.
            for i in 0..n {
                let base = prof_base + (i as u64) * (PROFILE_BYTES as u64);
                // ptype = 1.0
                f.instruction(&Ins::I32Const(0));
                f.instruction(&f64_const(1.0));
                f.instruction(&Ins::F64Store(memarg(base)));
                // ppriority = priorities[i] or 0.0
                f.instruction(&Ins::I32Const(0));
                if i < pri_size {
                    emit_view_element_load(priority_view, i, ctx, f)?;
                } else {
                    f.instruction(&f64_const(0.0));
                }
                f.instruction(&Ins::F64Store(memarg(base + 8)));
                // pwidth = width (runtime)
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::LocalGet(width));
                f.instruction(&Ins::F64Store(memarg(base + 16)));
                // pextra = 0.0
                f.instruction(&Ins::I32Const(0));
                f.instruction(&f64_const(0.0));
                f.instruction(&Ins::F64Store(memarg(base + 24)));
            }

            // allocate_available(req_base, n, prof_base, supply, out_base)
            f.instruction(&Ins::I32Const(req_base as i32));
            f.instruction(&Ins::I32Const(n as i32));
            f.instruction(&Ins::I32Const(prof_base as i32));
            f.instruction(&Ins::LocalGet(supply));
            f.instruction(&Ins::I32Const(out_base as i32));
            f.instruction(&Ins::Call(ctx.helpers.allocate_available));

            emit_copy_out_to_temp(out_base, n, write_temp_id, ctx, f)
        },
    )
}

/// The three consecutive scratch sub-region byte bases for an allocation of `n`
/// requesters: `requests` (n f64) at `alloc_scratch_base`, `profiles` (4n f64)
/// after it, `out` (n f64) after that. All three are live across the
/// `allocate_available` call; `module.rs` sizes the region for the largest `n`.
fn alloc_scratch_layout(ctx: &EmitCtx, n: usize) -> (u64, u64, u64) {
    let base = u64::from(ctx.alloc_scratch_base);
    let req_base = base;
    let prof_base = req_base + (n as u64) * u64::from(SLOT_SIZE);
    let out_base = prof_base + (n as u64) * (PROFILE_BYTES as u64);
    (req_base, prof_base, out_base)
}

/// Copy the `n` allocations the helper wrote at `out_base` into temp
/// `write_temp_id` (`temp[temp_off + i] = out[i]`). Unrolled over `n`.
fn emit_copy_out_to_temp(
    out_base: u64,
    n: usize,
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    for i in 0..n {
        let temp_addr = temp_element_byte_addr(ctx, write_temp_id, i as u32)?;
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(
            out_base + (i as u64) * u64::from(SLOT_SIZE),
        )));
        f.instruction(&Ins::F64Store(memarg(temp_addr)));
    }
    Ok(())
}

/// Emit `body` gated on the VM's "`!is_valid` -> fill_temp_nan" short-circuit
/// for the allocation arms. When no input view carries a runtime validity flag
/// (the common static/temp/full-var case), `body` is emitted directly with no
/// runtime check; otherwise `if all_valid { body } else { fill_temp_nan }`.
/// Mirrors `super::vector::emit_with_validity_gate`.
fn emit_with_validity_gate(
    views: &[&ViewDesc],
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
    body: impl FnOnce(&EmitCtx, &mut Function) -> Result<(), WasmGenError>,
) -> Result<(), WasmGenError> {
    let valids: Vec<u32> = views.iter().filter_map(|v| v.valid_local).collect();
    if valids.is_empty() {
        return body(ctx, f);
    }
    f.instruction(&Ins::LocalGet(valids[0]));
    for &v in &valids[1..] {
        f.instruction(&Ins::LocalGet(v));
        f.instruction(&Ins::I32And);
    }
    f.instruction(&Ins::If(BlockType::Empty));
    body(ctx, f)?;
    f.instruction(&Ins::Else);
    emit_fill_temp_nan(ctx, write_temp_id, f)?;
    f.instruction(&Ins::End);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::lower::build_helpers;
    use super::f64_memarg_off;
    use checked::Store;
    use wasm::validate;
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction as Ins,
        MemorySection, MemoryType, Module, TypeSection, ValType,
    };

    // The allocation helpers are bit-faithful ports of `crate::alloc`. Their
    // leaf transcendental helpers (`exp`/`pow`) are NOT bit-identical to the
    // VM's libm -- they are the open-coded approximations of Phase 2, pinned in
    // `super::super::math` to abs 0.0 / rel ~1e-12 vs `f64`. So the alloc helpers
    // can only match the Rust `crate::alloc` reference (which uses libm) to that
    // leaf tolerance, propagated through the curves and the bisection.
    //
    // Documented tolerances (all far inside the corpus bar of abs 2e-3 /
    // rel 5e-6):
    // - erfc_approx / normal_cdf: abs 1e-12 OR rel 1e-12. erfc's only
    //   transcendental is one `exp` call (rel ~1e-12); the polynomial is exact
    //   arithmetic, so the wasm result tracks `crate::alloc::erfc_approx` to the
    //   exp helper's tolerance.
    // - alloc_curve: abs 1e-9 OR rel 1e-9 across all six branches. Most use at
    //   most one exp/normal_cdf (rel ~1e-12); CES adds a `pow = exp(y*ln x)`
    //   (pinned at rel ~2.3e-12). The uniform 1e-9 bar leaves ample slack for
    //   the leaf approximations + DLR-FT-vs-native rounding drift.
    // - allocate_available: abs 1e-9 OR rel 1e-9 -- the converged price rides on
    //   the curve tolerance, and the per-requester allocation is one more curve
    //   evaluation at that price.
    const ERFC_ABS: f64 = 1e-12;
    const ERFC_REL: f64 = 1e-12;
    const CURVE_ABS: f64 = 1e-9;
    const CURVE_REL: f64 = 1e-9;
    const ALLOC_ABS: f64 = 1e-9;
    const ALLOC_REL: f64 = 1e-9;

    /// Assert `got` matches `want` within absolute *or* relative tolerance,
    /// propagating NaN/inf. Mirrors `super::super::math`'s `assert_close`.
    fn assert_close(name: &str, got: f64, want: f64, abs_tol: f64, rel_tol: f64) {
        if want.is_nan() {
            assert!(got.is_nan(), "{name}: expected NaN, got {got}");
            return;
        }
        assert!(!got.is_nan(), "{name}: got NaN, expected {want}");
        if want.is_infinite() {
            assert_eq!(got, want, "{name}: expected {want}, got {got}");
            return;
        }
        let abs = (got - want).abs();
        let rel = if want != 0.0 { abs / want.abs() } else { abs };
        assert!(
            abs <= abs_tol || rel <= rel_tol,
            "{name}: got {got}, want {want} (abs {abs:.3e}, rel {rel:.3e})"
        );
    }

    /// A linear sample of `n+1` points across `[lo, hi]` inclusive.
    fn linspace(lo: f64, hi: f64, n: usize) -> Vec<f64> {
        (0..=n)
            .map(|i| lo + (hi - lo) * (i as f64) / (n as f64))
            .collect()
    }

    /// Which value-producing alloc helper a test module exports as `f`.
    ///
    /// The DLR-FT interop only types tuples up to arity 3, so the unary helpers
    /// (`Erfc`/`NormalCdf`) export `f(x: f64) -> f64` directly, while the
    /// six-argument `AllocCurve` exports `f(args_ptr: i32) -> f64` and reads its
    /// six f64 arguments from `mem[args_ptr + k*8]`.
    #[derive(Clone, Copy)]
    enum Which {
        Erfc,
        NormalCdf,
        AllocCurve,
    }

    fn helper_index(which: Which) -> u32 {
        let h = build_helpers().fns;
        match which {
            Which::Erfc => h.erfc_approx,
            Which::NormalCdf => h.normal_cdf,
            Which::AllocCurve => h.alloc_curve,
        }
    }

    /// Build a module with every helper body plus a thin exported `f` forwarding
    /// to the helper under test, and a memory (the GF lookup helpers, also
    /// bundled, `f64.load` from memory 0). For a unary helper `f(x: f64) -> f64`
    /// calls directly; for `AllocCurve` (six args) `f(args_ptr: i32) -> f64`
    /// loads the six args from `mem[args_ptr + k*8]` and calls the helper.
    /// Mirrors `super::super::math`'s `build_helper_module` layout (helpers at
    /// `0..N`, wrapper at `N`).
    fn build_value_module(which: Which) -> Vec<u8> {
        let helpers = build_helpers();
        let n_helpers = helpers.functions.len() as u32;
        let target = helper_index(which);
        let is_curve = matches!(which, Which::AllocCurve);

        let mut module = Module::new();

        let mut types = TypeSection::new();
        if is_curve {
            types.ty().function([ValType::I32], [ValType::F64]);
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
        exports.export("mem", ExportKind::Memory, 0);
        module.section(&exports);

        let mut code = CodeSection::new();
        for hf in &helpers.functions {
            code.function(&hf.body);
        }
        let mut wrapper = Function::new([]);
        if is_curve {
            // Load the six f64 args from mem[args_ptr + k*8] (args_ptr is param 0).
            for k in 0..6u64 {
                wrapper.instruction(&Ins::LocalGet(0));
                wrapper.instruction(&Ins::F64Load(f64_memarg_off(k * 8)));
            }
        } else {
            wrapper.instruction(&Ins::LocalGet(0));
        }
        wrapper.instruction(&Ins::Call(target));
        wrapper.instruction(&Ins::End);
        code.function(&wrapper);
        module.section(&code);

        module.finish()
    }

    fn run_unary(which: Which, x: f64) -> f64 {
        let bytes = build_value_module(which);
        let info = validate(&bytes).expect("helper module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let f = store
            .instance_export(module, "f")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(f64,), f64>(f, (x,))
            .expect("invoke")
    }

    /// Byte address the `AllocCurve` wrapper reads its six f64 args from.
    const CURVE_ARGS_BASE: u32 = 512;

    /// Run `alloc_curve(p, request, ptype, ppriority, pwidth, pextra)` under the
    /// interpreter. The six args are seeded into memory at [`CURVE_ARGS_BASE`]
    /// (`ptype` as an integer-valued f64) and the wrapper reads them back.
    fn run_alloc_curve(p: f64, request: f64, ptype: i32, pp: f64, pw: f64, pe: f64) -> f64 {
        let bytes = build_value_module(Which::AllocCurve);
        let info = validate(&bytes).expect("alloc_curve module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let args = [p, request, ptype as f64, pp, pw, pe];
        let mem = store
            .instance_export(module, "mem")
            .unwrap()
            .as_mem()
            .unwrap();
        store.mem_access_mut_slice(mem, |b| {
            for (k, &v) in args.iter().enumerate() {
                let a = CURVE_ARGS_BASE as usize + k * 8;
                b[a..a + 8].copy_from_slice(&v.to_le_bytes());
            }
        });
        let f = store
            .instance_export(module, "f")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(i32,), f64>(f, (CURVE_ARGS_BASE as i32,))
            .expect("invoke")
    }

    // ── erfc_approx parity vs crate::alloc::erfc_approx (AC7.1) ──────────────

    #[test]
    fn erfc_approx_matches_rust_over_sampled_range() {
        // Sweep both signs (z<0 takes the `2 - erfc_approx(-z)` symmetry branch)
        // across the range where erfc is numerically interesting; the A-S 26.2.17
        // approximation is what the Rust reference uses too, so the wasm result
        // tracks it to the `exp` helper's tolerance.
        for z in linspace(-6.0, 6.0, 400) {
            let got = run_unary(Which::Erfc, z);
            let want = crate::alloc::erfc_approx(z);
            assert_close(&format!("erfc_approx({z})"), got, want, ERFC_ABS, ERFC_REL);
        }
        // Anchor at z=0 (t=1): the wasm result tracks the Rust reference, which
        // is ~0.9999999990 there -- the A-S 26.2.17 approximation, not the
        // mathematical erfc(0)=1.
        assert_close(
            "erfc_approx(0)",
            run_unary(Which::Erfc, 0.0),
            crate::alloc::erfc_approx(0.0),
            ERFC_ABS,
            ERFC_REL,
        );
    }

    // ── normal_cdf parity vs crate::alloc::normal_cdf (AC7.1) ────────────────

    #[test]
    fn normal_cdf_matches_rust_over_sampled_range() {
        for x in linspace(-6.0, 6.0, 400) {
            let got = run_unary(Which::NormalCdf, x);
            let want = crate::alloc::normal_cdf(x);
            assert_close(&format!("normal_cdf({x})"), got, want, ERFC_ABS, ERFC_REL);
        }
        // NaN propagates (the explicit `x.is_nan()` guard).
        assert!(run_unary(Which::NormalCdf, f64::NAN).is_nan());
        // normal_cdf(0) tracks the Rust reference. (The A-S 26.2.17 erfc
        // polynomial is ~0.4999999995 at x=0, NOT exactly 0.5 -- the ~1.5e-7
        // approximation error is a property of the reference itself, so parity
        // is judged against `crate::alloc::normal_cdf`, not ideal math.)
        assert_close(
            "normal_cdf(0)",
            run_unary(Which::NormalCdf, 0.0),
            crate::alloc::normal_cdf(0.0),
            ERFC_ABS,
            ERFC_REL,
        );
    }

    // ── alloc_curve parity for each of the 6 profile types + the >=10 floor ──

    /// Assert the emitted `alloc_curve` matches `crate::alloc::alloc_curve` over
    /// a grid of prices for one profile `(ptype, ppriority, pwidth, pextra)` and
    /// a fixed positive request.
    fn assert_curve_matches(ptype: i32, pp: f64, pw: f64, pe: f64, request: f64) {
        for p in linspace(-3.0, 8.0, 120) {
            let got = run_alloc_curve(p, request, ptype, pp, pw, pe);
            let want = crate::alloc::alloc_curve(p, request, ptype, pp, pw, pe);
            assert_close(
                &format!("alloc_curve(p={p}, ptype={ptype}, pp={pp}, pw={pw}, pe={pe})"),
                got,
                want,
                CURVE_ABS,
                CURVE_REL,
            );
        }
    }

    #[test]
    fn alloc_curve_fixed_matches_rust() {
        // ptype 0: fixed quantity (p <= ppriority ? request : 0).
        assert_curve_matches(0, 2.0, 1.0, 0.0, 5.0);
    }

    #[test]
    fn alloc_curve_rectangular_matches_rust() {
        // ptype 1: rectangular survival.
        assert_curve_matches(1, 3.0, 1.5, 0.0, 4.0);
    }

    #[test]
    fn alloc_curve_triangular_matches_rust() {
        // ptype 2: triangular survival (both p<=ppriority and p>ppriority arms).
        assert_curve_matches(2, 2.5, 2.0, 0.0, 7.0);
    }

    #[test]
    fn alloc_curve_normal_matches_rust() {
        // ptype 3: normal survival via normal_cdf. Also exercise the pwidth<=0
        // degenerate-to-fixed arm.
        assert_curve_matches(3, 2.0, 1.0, 0.0, 6.0);
        assert_curve_matches(3, 2.0, 0.0, 0.0, 6.0); // pwidth <= 0 -> fixed
    }

    #[test]
    fn alloc_curve_exponential_matches_rust() {
        // ptype 4: symmetric exponential (both z>0 and z<=0 arms). Also the
        // pwidth<=0 degenerate-to-fixed arm.
        assert_curve_matches(4, 2.0, 1.0, 0.0, 8.0);
        assert_curve_matches(4, 2.0, -1.0, 0.0, 8.0); // pwidth <= 0 -> fixed
    }

    #[test]
    fn alloc_curve_ces_matches_rust() {
        // ptype 5: CES (uses pow). pextra is the elasticity. The grid spans
        // p<=0 (->1), ppriority>0 normal case, and large-elasticity values that
        // push q toward +inf (->1).
        assert_curve_matches(5, 3.0, 1.0, 1.0, 5.0);
        assert_curve_matches(5, 3.0, 1.0, 4.0, 5.0);
        // ppriority <= 0 -> 0 for any positive price.
        assert_curve_matches(5, 0.0, 1.0, 2.0, 5.0);
    }

    #[test]
    fn alloc_curve_floor_flag_matches_rust() {
        // ptype >= 10 floors the allocation. ptype 10 is rectangular(0)+floor,
        // 11 is rectangular(1)+floor, etc. Pick a request that yields a
        // fractional allocation so the floor is observable.
        for ptype in [10, 11, 13, 14, 15] {
            assert_curve_matches(ptype, 2.5, 1.5, 1.0, 3.3);
        }
    }

    #[test]
    fn alloc_curve_nonpositive_request_is_zero() {
        // request <= 0 -> 0 for every profile, regardless of price/type.
        for &request in &[0.0, -1.0, -100.0] {
            for ptype in 0..6 {
                let got = run_alloc_curve(1.0, request, ptype, 2.0, 1.0, 1.0);
                let want = crate::alloc::alloc_curve(1.0, request, ptype, 2.0, 1.0, 1.0);
                assert_eq!(got, want, "request {request}, ptype {ptype}");
                assert_eq!(got, 0.0);
            }
        }
    }

    // ── allocate_available parity vs crate::alloc::allocate_available ────────

    // Scratch byte layout for the `allocate_available` helper test: the i32
    // requester count at N_ADDR, requests at REQ_BASE, profiles at PROF_BASE
    // (4 f64/requester), out at OUT_BASE. All 8-byte aligned (N_ADDR 4-byte),
    // comfortably inside the single 64 KiB memory page.
    const N_ADDR: u32 = 64;
    const REQ_BASE: u32 = 256;
    const PROF_BASE: u32 = 1024;
    const OUT_BASE: u32 = 4096;

    /// Build a module with every helper body plus an exported `alloc(avail: f64)`
    /// wrapper that calls `allocate_available(REQ_BASE, n, PROF_BASE, avail,
    /// OUT_BASE)` with the array pointers hard-coded to the test's scratch bases
    /// and `n` read from `mem[N_ADDR]` (an i32). A single f64 param keeps the
    /// wrapper inside the DLR-FT interop's typed-tuple arity limit; the array
    /// pointers and `n` are seeded into memory by the test.
    fn build_allocate_module() -> Vec<u8> {
        let helpers = build_helpers();
        let n_helpers = helpers.functions.len() as u32;
        let target = helpers.fns.allocate_available;

        let mut module = Module::new();

        let mut types = TypeSection::new();
        // alloc(avail: f64) -> ()
        types.ty().function([ValType::F64], []);
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
        exports.export("alloc", ExportKind::Func, n_helpers);
        exports.export("mem", ExportKind::Memory, 0);
        module.section(&exports);

        let mut code = CodeSection::new();
        for hf in &helpers.functions {
            code.function(&hf.body);
        }
        let mut wrapper = Function::new([]);
        // allocate_available(REQ_BASE, mem[N_ADDR] as i32, PROF_BASE, avail, OUT_BASE)
        wrapper.instruction(&Ins::I32Const(REQ_BASE as i32));
        wrapper.instruction(&Ins::I32Const(0));
        wrapper.instruction(&Ins::I32Load(wasm_encoder::MemArg {
            offset: u64::from(N_ADDR),
            align: 2,
            memory_index: 0,
        }));
        wrapper.instruction(&Ins::I32Const(PROF_BASE as i32));
        wrapper.instruction(&Ins::LocalGet(0)); // avail (f64 param)
        wrapper.instruction(&Ins::I32Const(OUT_BASE as i32));
        wrapper.instruction(&Ins::Call(target));
        wrapper.instruction(&Ins::End);
        code.function(&wrapper);
        module.section(&code);

        module.finish()
    }

    /// Run the emitted `allocate_available` over `requests`/`profiles` and read
    /// back the `n` result slots; compare against `crate::alloc::allocate_available`.
    fn assert_allocate_matches(requests: &[f64], profiles: &[(f64, f64, f64, f64)], avail: f64) {
        assert_eq!(requests.len(), profiles.len());
        let n = requests.len();
        let bytes = build_allocate_module();
        let info = validate(&bytes).expect("allocate module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;

        // Seed n, requests, and profiles into scratch memory.
        let mem = store
            .instance_export(inst, "mem")
            .unwrap()
            .as_mem()
            .unwrap();
        store.mem_access_mut_slice(mem, |b| {
            let na = N_ADDR as usize;
            b[na..na + 4].copy_from_slice(&(n as i32).to_le_bytes());
            for (i, &r) in requests.iter().enumerate() {
                let a = REQ_BASE as usize + i * 8;
                b[a..a + 8].copy_from_slice(&r.to_le_bytes());
            }
            for (i, &(pt, pp, pw, pe)) in profiles.iter().enumerate() {
                let base = PROF_BASE as usize + i * 32;
                b[base..base + 8].copy_from_slice(&pt.to_le_bytes());
                b[base + 8..base + 16].copy_from_slice(&pp.to_le_bytes());
                b[base + 16..base + 24].copy_from_slice(&pw.to_le_bytes());
                b[base + 24..base + 32].copy_from_slice(&pe.to_le_bytes());
            }
        });

        let alloc = store
            .instance_export(inst, "alloc")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(f64,), ()>(alloc, (avail,))
            .expect("invoke");

        let got: Vec<f64> = store.mem_access_mut_slice(mem, |b| {
            (0..n)
                .map(|i| {
                    let a = OUT_BASE as usize + i * 8;
                    f64::from_le_bytes(b[a..a + 8].try_into().unwrap())
                })
                .collect()
        });
        let want = crate::alloc::allocate_available(requests, profiles, avail);
        assert_eq!(want.len(), n);
        for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
            assert_close(
                &format!("allocate_available[{i}]"),
                g,
                w,
                ALLOC_ABS,
                ALLOC_REL,
            );
        }
    }

    #[test]
    fn allocate_available_full_grant_when_supply_exceeds_demand() {
        // avail >= total_demand: each requester gets r.max(0). A negative request
        // clamps to 0 (the `r.max(0.0)` arm).
        let requests = [3.0, 2.0, -1.0, 4.0];
        let profiles = [
            (1.0, 1.0, 1.0, 0.0),
            (1.0, 2.0, 1.0, 0.0),
            (1.0, 3.0, 1.0, 0.0),
            (1.0, 1.5, 1.0, 0.0),
        ];
        // total_demand = 3+2+4 = 9 (the negative request is excluded).
        assert_allocate_matches(&requests, &profiles, 100.0);
    }

    #[test]
    fn allocate_available_zeros_when_supply_nonpositive() {
        // avail <= 0: all zeros.
        let requests = [3.0, 2.0, 4.0];
        let profiles = [
            (1.0, 1.0, 1.0, 0.0),
            (1.0, 2.0, 1.0, 0.0),
            (1.0, 3.0, 1.0, 0.0),
        ];
        assert_allocate_matches(&requests, &profiles, 0.0);
        assert_allocate_matches(&requests, &profiles, -5.0);
    }

    #[test]
    fn allocate_available_partial_bisection_rectangular() {
        // The interesting case: 0 < avail < total_demand, so the bisection runs.
        // Rectangular profiles (ptype 1) with distinct priorities, mirroring the
        // `allocate.mdl` shape.
        let requests = [3.0, 2.0, 4.0];
        let profiles = [
            (1.0, 1.0, 1.0, 0.0),
            (1.0, 2.0, 1.0, 0.0),
            (1.0, 3.0, 1.0, 0.0),
        ];
        // total_demand = 9; supply 5 forces a partial allocation.
        for avail in [1.0, 3.0, 5.0, 7.0, 8.5] {
            assert_allocate_matches(&requests, &profiles, avail);
        }
    }

    #[test]
    fn allocate_available_partial_bisection_across_profile_types() {
        // Partial allocation with a mix of profile types, exercising the
        // search-range `spread` per type and the per-requester curve at the
        // converged price.
        let requests = [4.0, 3.0, 5.0, 2.0, 6.0];
        let profiles = [
            (0.0, 2.0, 1.0, 0.0), // fixed
            (2.0, 3.0, 1.5, 0.0), // triangular
            (3.0, 2.5, 1.0, 0.0), // normal
            (4.0, 2.0, 1.2, 0.0), // exponential
            (5.0, 3.0, 1.0, 2.0), // CES
        ];
        // total_demand = 20; sweep several partial supplies.
        for avail in [2.0, 6.0, 10.0, 15.0, 19.0] {
            assert_allocate_matches(&requests, &profiles, avail);
        }
    }

    #[test]
    fn allocate_available_empty_requesters_is_noop() {
        // n == 0: nothing is written (the helper returns immediately). Exercised
        // by passing zero requesters; the read-back loop covers zero slots, so
        // this simply must not trap.
        assert_allocate_matches(&[], &[], 10.0);
    }
}
