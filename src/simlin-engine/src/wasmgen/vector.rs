// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: each emitter appends a wasm instruction sequence for one
// vector-operation opcode, mirroring the matching VM arm element-for-element. No
// I/O; the only side effect is in `#[cfg(test)]` (which lives in `lower_tests.rs`
// alongside the rest of the lowering harness).

//! Lowering of the bytecode VM's vector-operation opcodes to WebAssembly
//! (Phase 6).
//!
//! These opcodes operate over the compile-time view stack (`super::views`) and
//! the operand stack and -- except [`VectorSelect`](emit_vector_select), which
//! reduces to one scalar -- write their result array to a `write_temp_id` region
//! of `temp_storage`. Each emitter reproduces the matching VM dispatch arm
//! element-for-element:
//!
//! - [`emit_vector_select`] -- `vm.rs:2444-2502`
//! - [`emit_vector_elm_map`] -- `crate::vm_vector_elm_map::vector_elm_map`
//! - [`emit_vector_sort_order`] -- `crate::vm_vector_sort_order::vector_sort_order`
//! - [`emit_rank`] -- `vm.rs:2540-2584`
//! - [`emit_lookup_array`] -- `vm.rs:2586-2629`
//!
//! ## Runtime loop vs unrolled
//!
//! The *stable sort* ([`emit_stable_sort`], backing `VectorSortOrder`/`Rank`) is
//! a self-contained wasm helper with a **runtime** insertion-sort loop -- never
//! unrolled, since an unrolled O(n^2) body over a runtime view size would blow
//! up. Everything else here is a per-element map/gather/scatter over the
//! *compile-time* view size, so the element addresses fold into wasm constants
//! and the bodies are unrolled. The caller (`super::lower`) charges the Phase-5
//! [`EmitState`](super::lower) unroll budget for the view size before invoking
//! these, so the size cap still bounds an over-large arrayed model.
//!
//! ## Invalid input view
//!
//! An input view that a dynamic subscript (Phase-5 Task 4) made invalid at
//! runtime takes the VM's short-circuit: the whole destination temp region is
//! filled with IEEE `f64::NAN` (NOT the finite `crate::float::NA` sentinel) via
//! [`super::lower::emit_fill_temp_nan`], while `VectorSelect` pushes a single
//! NaN. The validity gate is only emitted when an input view actually carries a
//! runtime validity flag; in the common case (static / temp / full-var views)
//! every input is statically valid and no runtime check is generated.

use wasm_encoder::{BlockType, Function, Instruction as Ins, ValType};

use crate::bytecode::{GraphicalFunctionId, LookupMode};

use super::WasmGenError;
use super::lower::{
    EmitCtx, GF_DIRECTORY_ENTRY_BYTES, SLOT_SIZE, emit_fill_temp_nan, emit_is_truthy,
    emit_view_element_load, f64_const, i32_memarg, memarg, push_module_relative_base,
    temp_element_byte_addr,
};
use super::views::{ViewBase, ViewDesc};

/// Push `round_half_away(x)` for the f64 already on the wasm stack, reproducing
/// Rust's `f64::round` (round half AWAY from zero) bit-for-bit -- which is what
/// the VM uses (`stack.pop().round()`, `offset_val.round()`). This is NOT wasm
/// `f64.nearest` (round half to EVEN), so the two diverge for half-integer
/// inputs.
///
/// Emits the precision-safe form `t = x.trunc(); if (x - t).abs() >= 0.5 then t
/// plus-or-minus 1 (sign of x) else t`. The naive `trunc(x + copysign(0.5, x))`
/// is off-by-one against `f64::round` for two reachable input classes. First:
/// the largest f64 below 0.5 (`0.49999999999999994` and its negative), where
/// `x + 0.5` rounds up to exactly 1.0 so `trunc` yields a magnitude of one
/// though `f64::round` yields zero. Second: already-integer magnitudes in
/// `[2^52, 2^53)`, where `x + 0.5` rounds up to `x + 1` though `f64::round`
/// returns `x`. The `(x - t)` fraction here is computed exactly (the operands
/// are within a factor of two for `|x| < 2^53`, and `t == x` for integer
/// magnitudes at or above `2^52`), so no rounding can perturb the half-way
/// test. Verified bit-identical to `f64::round` over 5M random doubles
/// including sign-of-zero and both boundary classes.
///
/// `x_scratch` and `t_scratch` are two free f64 locals (distinct), holding `x`
/// and `trunc(x)` while each is read more than once.
pub(crate) fn emit_round_half_away(f: &mut Function, x_scratch: u32, t_scratch: u32) {
    f.instruction(&Ins::LocalSet(x_scratch)); // x_scratch = x
    f.instruction(&Ins::LocalGet(x_scratch));
    f.instruction(&Ins::F64Trunc);
    f.instruction(&Ins::LocalSet(t_scratch)); // t_scratch = trunc(x)

    // round-up value: t + copysign(1.0, x)  (the deeper Select operand)
    f.instruction(&Ins::LocalGet(t_scratch));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(x_scratch));
    f.instruction(&Ins::F64Copysign); // copysign(1.0, x): ±1.0 with x's sign
    f.instruction(&Ins::F64Add); // t + copysign(1.0, x)

    // keep-trunc value: t  (the shallower Select operand)
    f.instruction(&Ins::LocalGet(t_scratch));

    // condition: |x - t| >= 0.5  (exact fraction; round half away from zero)
    f.instruction(&Ins::LocalGet(x_scratch));
    f.instruction(&Ins::LocalGet(t_scratch));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Abs);
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::F64Ge);

    // select([round_up, t, cond]) == round_up when cond != 0, else t.
    f.instruction(&Ins::Select);
}

// ── stable sort helper (VectorSortOrder / Rank) ─────────────────────────────

// `stable_sort(pairs_ptr: i32, n: i32, ascending: i32)` local layout.
const SS_PTR: u32 = 0; // i32 byte address of pair 0
const SS_N: u32 = 1; // i32 pair count
const SS_ASC: u32 = 2; // i32 1 = ascending, else descending
const SS_I: u32 = 3; // i32 outer index
const SS_J: u32 = 4; // i32 inner index
const SS_KEY_VAL: u32 = 5; // f64 key value
const SS_KEY_IDX: u32 = 6; // f64 key idx payload
const SS_LEFT_VAL: u32 = 7; // f64 the left neighbour's value

/// Bytes per `(value: f64, idx: f64)` sort pair.
const PAIR_BYTES: i32 = 16;

/// Build the body of `stable_sort(pairs_ptr: i32, n: i32, ascending: i32) -> ()`,
/// an in-place **stable** insertion sort of `n` `(value: f64 @ +0, idx: f64 @ +8)`
/// pairs starting at byte `pairs_ptr`, ordered by `value`.
///
/// Reproduces the VM's stable `sort_by(|a, b| a.partial_cmp(b).unwrap_or(Equal))`
/// (ascending) / the `b.partial_cmp(a)` form (descending). The shift predicate is
/// a **strict** `f64.lt` (ascending) / `f64.gt` (descending) of the left
/// neighbour against the key: it is `false` whenever either operand is NaN, so a
/// NaN never displaces a non-NaN and never reorders relative to another NaN --
/// i.e. NaN comparisons act as `Equal`, exactly matching `partial_cmp(..)
/// .unwrap_or(Equal)` under a stable sort. Insertion sort only shifts past
/// strictly-ordered neighbours, so equal-keyed elements keep their input order
/// (stability) for free.
///
/// A runtime loop (never unrolled): `n` is a runtime view size, so an unrolled
/// O(n^2) body would be unbounded. n is small for real arrays (the corpus's
/// largest single dimension is 9), so insertion sort is more than adequate.
pub(crate) fn emit_stable_sort() -> Function {
    // Locals after the three i32 params: i32 SS_I/SS_J, f64 SS_KEY_VAL/
    // SS_KEY_IDX/SS_LEFT_VAL.
    let mut f = Function::new([(2, ValType::I32), (3, ValType::F64)]);

    // i = 1
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::LocalSet(SS_I));

    f.instruction(&Ins::Block(BlockType::Empty)); // $outer_exit
    f.instruction(&Ins::Loop(BlockType::Empty)); // $outer

    // while-head: if !(i < n) break $outer_exit  (br depth 1)
    f.instruction(&Ins::LocalGet(SS_I));
    f.instruction(&Ins::LocalGet(SS_N));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    // key_val = mem[ptr + 16*i + 0]; key_idx = mem[ptr + 16*i + 8]
    push_pair_addr(&mut f, SS_I);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalSet(SS_KEY_VAL));
    push_pair_addr(&mut f, SS_I);
    f.instruction(&Ins::F64Load(memarg(8)));
    f.instruction(&Ins::LocalSet(SS_KEY_IDX));

    // j = i - 1
    f.instruction(&Ins::LocalGet(SS_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(SS_J));

    f.instruction(&Ins::Block(BlockType::Empty)); // $inner_exit
    f.instruction(&Ins::Loop(BlockType::Empty)); // $inner

    // while-head: if !(j >= 0) break $inner_exit  (br depth 1)
    f.instruction(&Ins::LocalGet(SS_J));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    // left_val = mem[ptr + 16*j + 0]
    push_pair_addr(&mut f, SS_J);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalSet(SS_LEFT_VAL));

    // cmp = ascending ? (left_val > key_val) : (left_val < key_val)
    // Both are strict, hence false for any NaN operand (NaN-as-Equal stability).
    f.instruction(&Ins::LocalGet(SS_LEFT_VAL));
    f.instruction(&Ins::LocalGet(SS_KEY_VAL));
    f.instruction(&Ins::F64Gt); // gt (the ascending predicate)
    f.instruction(&Ins::LocalGet(SS_LEFT_VAL));
    f.instruction(&Ins::LocalGet(SS_KEY_VAL));
    f.instruction(&Ins::F64Lt); // lt (the descending predicate)
    f.instruction(&Ins::LocalGet(SS_ASC));
    f.instruction(&Ins::Select); // gt if ascending else lt
    // if !cmp break $inner_exit  (br depth 1)
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    // mem[ptr + 16*(j+1)] = mem[ptr + 16*j]  (both value and idx)
    push_pair_addr_plus1(&mut f, SS_J); // dst addr (value)
    push_pair_addr(&mut f, SS_J);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Store(memarg(0)));
    push_pair_addr_plus1(&mut f, SS_J); // dst addr (idx)
    push_pair_addr(&mut f, SS_J);
    f.instruction(&Ins::F64Load(memarg(8)));
    f.instruction(&Ins::F64Store(memarg(8)));

    // j -= 1 ; continue $inner
    f.instruction(&Ins::LocalGet(SS_J));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(SS_J));
    f.instruction(&Ins::Br(0));

    f.instruction(&Ins::End); // end $inner loop
    f.instruction(&Ins::End); // end $inner_exit block

    // mem[ptr + 16*(j+1)] = (key_val, key_idx)
    push_pair_addr_plus1(&mut f, SS_J);
    f.instruction(&Ins::LocalGet(SS_KEY_VAL));
    f.instruction(&Ins::F64Store(memarg(0)));
    push_pair_addr_plus1(&mut f, SS_J);
    f.instruction(&Ins::LocalGet(SS_KEY_IDX));
    f.instruction(&Ins::F64Store(memarg(8)));

    // i += 1 ; continue $outer
    f.instruction(&Ins::LocalGet(SS_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(SS_I));
    f.instruction(&Ins::Br(0));

    f.instruction(&Ins::End); // end $outer loop
    f.instruction(&Ins::End); // end $outer_exit block
    f.instruction(&Ins::End); // end function
    f
}

/// Push the byte address of sort pair `idx_local`: `ptr + 16 * idx`. A following
/// `f64.load`/`store` reads `value` at `memarg(0)` and `idx` at `memarg(8)`.
fn push_pair_addr(f: &mut Function, idx_local: u32) {
    f.instruction(&Ins::LocalGet(SS_PTR));
    f.instruction(&Ins::LocalGet(idx_local));
    f.instruction(&Ins::I32Const(PAIR_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
}

/// Push the byte address of sort pair `idx_local + 1`: `ptr + 16 * (idx + 1)`.
fn push_pair_addr_plus1(f: &mut Function, idx_local: u32) {
    f.instruction(&Ins::LocalGet(SS_PTR));
    f.instruction(&Ins::LocalGet(idx_local));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::I32Const(PAIR_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
}

// ── shared input-view helpers ───────────────────────────────────────────────

/// Whether `view` carries a runtime validity flag or runtime offset addend (a
/// dynamic subscript, Phase-5 Task 4).
///
/// This is the *`VectorSelect`-specific* dynamic-view rejection predicate (its
/// only consumer is [`is_dynamic_select`]). It deliberately keys on *both*
/// `valid_local` and `runtime_off_local` -- stricter than the temp-writers'
/// [`emit_with_validity_gate`], which keys on `valid_local` alone. The
/// difference is by design: `VectorSelect` reads its source via a compile-time-
/// base path that does NOT fold a runtime offset addend (it has no temp region
/// to gate and would need to thread the runtime offset into the gather by hand),
/// so any runtime offset disqualifies it. The temp-writers tolerate a
/// `runtime_off_local` because their element reads route through
/// [`emit_view_element_load`], which folds the runtime offset + validity itself.
fn is_dynamic(view: &ViewDesc) -> bool {
    view.valid_local.is_some() || view.runtime_off_local.is_some()
}

/// Push the i32 "all inputs valid" condition for `views`: the bitwise-AND of each
/// view's `valid_local`, or a constant `1` when no view carries one. Used to gate
/// the op against the VM's "`!is_valid` -> fill_temp_nan / NaN" short-circuit.
fn push_all_valid(views: &[&ViewDesc], f: &mut Function) {
    let valids: Vec<u32> = views.iter().filter_map(|v| v.valid_local).collect();
    if valids.is_empty() {
        f.instruction(&Ins::I32Const(1));
        return;
    }
    f.instruction(&Ins::LocalGet(valids[0]));
    for &v in &valids[1..] {
        f.instruction(&Ins::LocalGet(v));
        f.instruction(&Ins::I32And);
    }
}

/// The constant base byte address of `view`'s *storage element 0* -- i.e. the
/// address the VM's `read_view_element(view, flat)` indexes as `base + flat` (the
/// view's `base_off`, NOT folding in its `offset`, which the caller already folds
/// into the flat index). For a module-relative var view the runtime `module_off`
/// addend is signalled via the returned `bool`.
fn view_storage_base(view: &ViewDesc, ctx: &EmitCtx) -> Result<(u64, bool), WasmGenError> {
    match view.base {
        ViewBase::CurrAbsolute => Ok((
            u64::from(ctx.curr_base) + u64::from(view.base_off) * u64::from(SLOT_SIZE),
            false,
        )),
        ViewBase::CurrModuleRelative => Ok((
            u64::from(ctx.curr_base) + u64::from(view.base_off) * u64::from(SLOT_SIZE),
            true,
        )),
        ViewBase::Temp => {
            let temp_off = *ctx
                .ctx
                .temp_offsets
                .get(view.base_off as usize)
                .ok_or_else(|| {
                    WasmGenError::Unsupported(
                        "wasmgen: vector-op source references an out-of-range temp id".to_string(),
                    )
                })? as u64;
            Ok((
                u64::from(ctx.temp_storage_base) + temp_off * u64::from(SLOT_SIZE),
                false,
            ))
        }
    }
}

// ── VectorSelect (vm.rs:2444-2502) ──────────────────────────────────────────

/// Lower `VectorSelect`, mirroring `vm.rs:2444-2502`. The two operands are on the
/// wasm stack as `[max_value, action]` (`action` on top, matching the VM popping
/// `action` then `max_value`); the views are `expr_view = top`, `sel_view =
/// top-1`. Zips the two views to `min(sel.size, expr.size)` with independent
/// odometers, collects each `expr` value where `is_truthy(sel)`, then for an
/// empty selection pushes `max_value`, else dispatches the `action` reduction
/// (1=min, 2=mean, 3=max, 4=product, else sum). The single scalar result is left
/// on the stack. An invalid input view pushes one NaN.
///
/// The gather is unrolled over the (compile-time) zip size; each selected value
/// is appended to the vector scratch region with a runtime count, and the
/// reduction is a single runtime pass over the collected values (mirroring the
/// VM's `selected` Vec). `min`/`max` reproduce Rust's `f64::min`/`f64::max`
/// (NaN-ignoring), not wasm `f64.min`/`f64.max` (NaN-propagating), so the fold
/// matches the VM's `fold(±inf, f64::min/max)` exactly.
pub(crate) fn emit_vector_select(
    sel_view: &ViewDesc,
    expr_view: &ViewDesc,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    if is_dynamic_select(sel_view, expr_view) {
        return Err(WasmGenError::Unsupported(
            "wasmgen: VectorSelect over a dynamically-subscripted view is not supported"
                .to_string(),
        ));
    }

    let max_value = ctx.apply_locals[0]; // popped second
    let action = ctx.vector_i32_locals[0];
    let count = ctx.vector_i32_locals[1];
    let k = ctx.vector_i32_locals[2];
    let [acc_sum, acc_prod, acc_min, acc_max, vtmp] = ctx.vector_f64_locals;

    // Pop action (top) -> round-half-away -> i32; then pop max_value. The round
    // uses `scratch_local` + `apply_locals[0]` as its two f64 temps; both are
    // free here (`max_value` is parked into `apply_locals[0]` only afterward).
    emit_round_half_away(f, ctx.scratch_local, ctx.apply_locals[0]);
    f.instruction(&Ins::I32TruncSatF64S);
    f.instruction(&Ins::LocalSet(action));
    f.instruction(&Ins::LocalSet(max_value));

    let size = sel_view.size().min(expr_view.size());

    // count = 0
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(count));

    // Gather: for each i in 0..size, if is_truthy(sel[i]) push expr[i] into the
    // scratch region at scratch[count] and bump count. The two odometers run
    // independently; element `i` of each view is its row-major iteration index.
    for i in 0..size {
        emit_view_element_load(sel_view, i, ctx, f)?;
        emit_is_truthy(ctx, f);
        f.instruction(&Ins::If(BlockType::Empty));
        // scratch[count] = expr[i]. f64.store wants [addr_i32, value_f64];
        // addr = vector_scratch_base + count*8 (the constant base in memarg).
        f.instruction(&Ins::LocalGet(count));
        f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
        f.instruction(&Ins::I32Mul);
        emit_view_element_load(expr_view, i, ctx, f)?;
        f.instruction(&Ins::F64Store(memarg(u64::from(ctx.vector_scratch_base))));
        // count += 1
        f.instruction(&Ins::LocalGet(count));
        f.instruction(&Ins::I32Const(1));
        f.instruction(&Ins::I32Add);
        f.instruction(&Ins::LocalSet(count));
        f.instruction(&Ins::End);
    }

    // if count == 0 { result = max_value } else { result = reduce(action) }.
    f.instruction(&Ins::LocalGet(count));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
    f.instruction(&Ins::LocalGet(max_value));
    f.instruction(&Ins::Else);

    // Single pass over scratch[0..count] computing sum/product/min/max; then the
    // action selects the result. min/max init mirror the VM's
    // fold(INFINITY, f64::min) / fold(NEG_INFINITY, f64::max).
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(acc_sum));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalSet(acc_prod));
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::LocalSet(acc_min));
    f.instruction(&f64_const(f64::NEG_INFINITY));
    f.instruction(&Ins::LocalSet(acc_max));
    // k = 0
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(k));

    f.instruction(&Ins::Block(BlockType::Empty)); // $reduce_exit
    f.instruction(&Ins::Loop(BlockType::Empty)); // $reduce
    // if !(k < count) break
    f.instruction(&Ins::LocalGet(k));
    f.instruction(&Ins::LocalGet(count));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));
    // v = scratch[k]
    f.instruction(&Ins::LocalGet(k));
    f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::F64Load(memarg(u64::from(ctx.vector_scratch_base))));
    f.instruction(&Ins::LocalSet(vtmp));
    // acc_sum += v
    f.instruction(&Ins::LocalGet(acc_sum));
    f.instruction(&Ins::LocalGet(vtmp));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(acc_sum));
    // acc_prod *= v
    f.instruction(&Ins::LocalGet(acc_prod));
    f.instruction(&Ins::LocalGet(vtmp));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(acc_prod));
    // acc_min = f64::min(acc_min, v)
    f.instruction(&Ins::LocalGet(acc_min));
    f.instruction(&Ins::LocalGet(vtmp));
    emit_f64_min_rust(ctx, f);
    f.instruction(&Ins::LocalSet(acc_min));
    // acc_max = f64::max(acc_max, v)
    f.instruction(&Ins::LocalGet(acc_max));
    f.instruction(&Ins::LocalGet(vtmp));
    emit_f64_max_rust(ctx, f);
    f.instruction(&Ins::LocalSet(acc_max));
    // k += 1 ; continue
    f.instruction(&Ins::LocalGet(k));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(k));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // end $reduce loop
    f.instruction(&Ins::End); // end $reduce_exit block

    // result = match action { 1 => min, 2 => sum/count, 3 => max, 4 => prod,
    //                         _ => sum }. wasm `select` pops [v1, v2, cond] and
    // yields the DEEPER `v1` when cond != 0, so the running default (`sum`) is the
    // deeper operand, each override is pushed shallower, and the condition is
    // `action != n` -- keeping the running value unless `action == n`. (Same
    // pattern as `math::emit_quadrant_select`.)
    f.instruction(&Ins::LocalGet(acc_sum)); // default: sum (action 0/5/..)
    // action == 4 -> product
    f.instruction(&Ins::LocalGet(acc_prod));
    push_action_ne(f, action, 4);
    f.instruction(&Ins::Select);
    // action == 3 -> max
    f.instruction(&Ins::LocalGet(acc_max));
    push_action_ne(f, action, 3);
    f.instruction(&Ins::Select);
    // action == 2 -> mean (sum / count)
    f.instruction(&Ins::LocalGet(acc_sum));
    f.instruction(&Ins::LocalGet(count));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Div);
    push_action_ne(f, action, 2);
    f.instruction(&Ins::Select);
    // action == 1 -> min
    f.instruction(&Ins::LocalGet(acc_min));
    push_action_ne(f, action, 1);
    f.instruction(&Ins::Select);

    f.instruction(&Ins::End); // end if count == 0
    Ok(())
}

/// `VectorSelect`'s dynamic-view rejection. The op reduces to a scalar (no temp
/// region), so an invalid view would push one NaN; rather than emit that gate
/// (and the runtime-offset folding the gather would need), a dynamically-
/// subscripted input is reported as `WasmGenError::Unsupported` for the caller
/// to surface as an explicit error (no silent VM fallback).
fn is_dynamic_select(sel_view: &ViewDesc, expr_view: &ViewDesc) -> bool {
    is_dynamic(sel_view) || is_dynamic(expr_view)
}

/// Push i32 `1` when the i32 in `action_local` does NOT equal `n` -- the "keep
/// the running default" condition for the `VectorSelect` action-dispatch selects
/// (the override is taken only when `action == n`).
fn push_action_ne(f: &mut Function, action_local: u32, n: i32) {
    f.instruction(&Ins::LocalGet(action_local));
    f.instruction(&Ins::I32Const(n));
    f.instruction(&Ins::I32Ne);
}

/// Push `f64::min(a, b)` for `[a, b]` on the wasm stack, reproducing Rust's
/// NaN-ignoring `f64::min` (return the non-NaN operand if exactly one is NaN, the
/// lesser otherwise) rather than wasm `f64.min` (NaN-propagating). Parks both
/// operands so they can be read for the NaN tests and the `<` compare.
fn emit_f64_min_rust(ctx: &EmitCtx, f: &mut Function) {
    emit_f64_minmax_rust(ctx, f, true);
}

/// Push `f64::max(a, b)` for `[a, b]` on the wasm stack, reproducing Rust's
/// NaN-ignoring `f64::max`.
fn emit_f64_max_rust(ctx: &EmitCtx, f: &mut Function) {
    emit_f64_minmax_rust(ctx, f, false);
}

/// Shared body of [`emit_f64_min_rust`]/[`emit_f64_max_rust`]. Consumes `[a, b]`
/// and pushes `f64::min(a,b)` (`want_min`) or `f64::max(a,b)`, matching
/// `f64::min`/`f64::max`'s "ignore NaN; if both NaN, NaN" contract.
///
/// Built as three nested `select`s, each in the wasm "deeper operand wins when
/// cond != 0" form (`select([v1, v2, cond]) == v1 if cond else v2`):
/// 1. `core = (a {<,>} b) ? a : b`  -- the non-NaN min/max,
/// 2. `r = (b is NaN) ? a : core`   -- ignore a NaN `b`,
/// 3. result `= (a is NaN) ? b : r` -- ignore a NaN `a` (and if both NaN, `b`,
///    which is NaN, so the all-NaN case yields NaN).
///
/// The intermediate must be a *shallower* select operand at each step, so it is
/// parked in a scratch local rather than left on the stack. The `VectorSelect`
/// reduction reaches this only inside its `count != 0` branch, where all three
/// `Apply` scratch f64s are free (`apply_locals[0]`'s `max_value` is dead once
/// the selection is non-empty); this uses `apply_locals[1]`/`[2]` for `a`/`b` and
/// `apply_locals[0]` for the running register. (The ±0 tie is left to wasm's
/// `<`/`>`, acceptable for SD parity -- the VM's reductions never depend on ±0.)
fn emit_f64_minmax_rust(ctx: &EmitCtx, f: &mut Function, want_min: bool) {
    let a = ctx.apply_locals[1];
    let b = ctx.apply_locals[2];
    let r = ctx.apply_locals[0];
    // The two operands are on the stack as [a, b] (b on top); park them.
    f.instruction(&Ins::LocalSet(b));
    f.instruction(&Ins::LocalSet(a));

    // core = (a {<,>} b) ? a : b  -> r
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
    f.instruction(&Ins::F64Ne); // b != b  (true iff b is NaN)
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalSet(r));

    // result = (a is NaN) ? b : r  (left on the stack)
    f.instruction(&Ins::LocalGet(b));
    f.instruction(&Ins::LocalGet(r));
    f.instruction(&Ins::LocalGet(a));
    f.instruction(&Ins::LocalGet(a));
    f.instruction(&Ins::F64Ne); // a != a  (true iff a is NaN)
    f.instruction(&Ins::Select);
}

// ── VectorElmMap (vm_vector_elm_map.rs:33-116) ──────────────────────────────

/// Lower `VectorElmMap { write_temp_id, full_source_len }`, mirroring
/// `crate::vm_vector_elm_map::vector_elm_map`. The views are `offset_view = top`,
/// `source_view = top-1`. For each element `i` of the offset view: `flat_i =
/// base_i + round(offset[i])` over the source's FULL row-major storage, where
/// `base_i` is 0 for a full contiguous source else the source's flat offset at
/// element `i`'s carried-axis projection (the offset-view indices scattered onto
/// the source axes by dim-id). The result is `NaN` if `offset[i]` is NaN or
/// `flat_i` is out of `[0, full_source_len)`, else `source[flat_i]`. **No
/// modulo.** Written to `temp[temp_off + i]`; an invalid input view fills the
/// whole destination temp region with NaN.
pub(crate) fn emit_vector_elm_map(
    source_view: &ViewDesc,
    offset_view: &ViewDesc,
    write_temp_id: u8,
    full_source_len: u32,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    // The source's runtime-indexed read folds a module-relative addend, but a
    // runtime-offset addend (a dynamic subscript) on the source is NOT folded
    // into the compile-time `base_i`, so reject a dynamically-subscripted source
    // (VM fallback). The OFFSET view's reads route through `emit_view_element_load`
    // (which handles a runtime offset + validity), and an invalid offset view is
    // caught by the op-level validity gate below, so an offset dynamic subscript
    // is fine.
    if source_view.runtime_off_local.is_some() {
        return Err(WasmGenError::Unsupported(
            "wasmgen: VectorElmMap over a dynamically-subscripted source view is not supported"
                .to_string(),
        ));
    }

    emit_with_validity_gate(
        &[source_view, offset_view],
        write_temp_id,
        ctx,
        f,
        |ctx, f| {
            emit_vector_elm_map_body(
                source_view,
                offset_view,
                write_temp_id,
                full_source_len,
                ctx,
                f,
            )
        },
    )
}

/// The valid-input body of [`emit_vector_elm_map`].
fn emit_vector_elm_map_body(
    source_view: &ViewDesc,
    offset_view: &ViewDesc,
    write_temp_id: u8,
    full_source_len: u32,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let full_len = full_source_len as usize;
    let offset_size = offset_view.size();

    // source_is_full_array: the fast path where base_i is hard-coded 0 and the
    // offset indexes the whole array directly (vm_vector_elm_map.rs:52).
    let source_is_full_array = source_view.size() == full_len && source_view.is_contiguous();

    // Carried source dim -> offset-view axis of the same dim id, mirroring the
    // VM's `src_to_off_axis` (vm_vector_elm_map.rs:57-61). Used per element to
    // project the offset-view indices onto the source axes for `base_i`.
    let src_to_off_axis: Vec<Option<usize>> = source_view
        .dim_ids
        .iter()
        .map(|sd| offset_view.dim_ids.iter().position(|od| od == sd))
        .collect();

    let (src_base_byte, src_module_relative) = view_storage_base(source_view, ctx)?;

    let offset_val = ctx.vector_f64_locals[0];
    let flat_i = ctx.vector_i32_locals[0];

    for i in 0..offset_size {
        // base_i (compile-time): 0 for a full-array source, else the sliced
        // view's flat offset at this element's carried-dim projection.
        let base_i: i64 = if source_is_full_array {
            0
        } else {
            let off_indices = ViewDesc::decompose_iter_index(&offset_view.dims, i);
            let src_indices: Vec<u16> = src_to_off_axis
                .iter()
                .map(|slot| match slot {
                    Some(p) => off_indices[*p],
                    None => 0,
                })
                .collect();
            source_view.flat_offset_for_indices(&src_indices) as i64
        };

        // offset_val = offset_view[i]
        emit_view_element_load(offset_view, i, ctx, f)?;
        f.instruction(&Ins::LocalSet(offset_val));

        // result = if offset_val.is_nan() || flat_i<0 || flat_i>=full_len { NaN }
        //          else source[flat_i]. flat_i = base_i + round(offset_val).
        // Compute flat_i (i32) once. The round consumes the pushed copy of
        // `offset_val` and uses `scratch_local` + `apply_locals[0]` as its two
        // f64 temps -- neither is `vector_f64_locals[0]` (the `offset_val` local,
        // read again below), and `apply_locals` is otherwise unused in this op.
        f.instruction(&f64_const(base_i as f64));
        f.instruction(&Ins::LocalGet(offset_val));
        emit_round_half_away(f, ctx.scratch_local, ctx.apply_locals[0]);
        f.instruction(&Ins::F64Add); // base_i + round(offset_val)  (as f64)
        f.instruction(&Ins::I32TruncSatF64S);
        f.instruction(&Ins::LocalSet(flat_i));

        // store temp[i] = select(NaN, source[flat_i], oob). oob is true when the
        // offset is NaN OR flat_i is out of [0, full_len). f64.store wants
        // [addr_i32, value_f64]; push the temp address first.
        let temp_addr = temp_element_byte_addr(ctx, write_temp_id, i as u32)?;
        f.instruction(&Ins::I32Const(0)); // dynamic addr part (const base in memarg)

        // value = read source[flat_i] (faithful even when oob -- the select
        // discards it; flat_i is sat-clamped so the address stays in range only
        // when in-bounds, but a read at a clamped OOB index is never used).
        // Guard the read with the in-bounds branch so an OOB index never loads
        // out of the source storage.
        f.instruction(&Ins::LocalGet(offset_val));
        f.instruction(&Ins::LocalGet(offset_val));
        f.instruction(&Ins::F64Ne); // offset_val is NaN
        f.instruction(&Ins::LocalGet(flat_i));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32LtS); // flat_i < 0
        f.instruction(&Ins::I32Or);
        f.instruction(&Ins::LocalGet(flat_i));
        f.instruction(&Ins::I32Const(full_len as i32));
        f.instruction(&Ins::I32GeS); // flat_i >= full_len
        f.instruction(&Ins::I32Or); // oob
        f.instruction(&Ins::If(BlockType::Result(ValType::F64)));
        f.instruction(&f64_const(f64::NAN));
        f.instruction(&Ins::Else);
        // source[flat_i]: base byte + flat_i*8 (+ module_off*8 if module-relative)
        emit_storage_indexed_load(src_base_byte, src_module_relative, flat_i, ctx, f);
        f.instruction(&Ins::End);

        f.instruction(&Ins::F64Store(memarg(temp_addr)));
    }
    Ok(())
}

/// Push `storage[flat_i]` where the storage element-0 byte address is the
/// constant `base_byte` and `flat_i` (an i32 local) is the runtime slot index:
/// `f64.load[base_byte + (module_off? )*8 + flat_i*8]`. The constant `base_byte`
/// rides in the `memarg.offset`; the runtime part is `(module_off + flat_i) * 8`
/// for a module-relative view, else `flat_i * 8`.
fn emit_storage_indexed_load(
    base_byte: u64,
    module_relative: bool,
    flat_i: u32,
    ctx: &EmitCtx,
    f: &mut Function,
) {
    if module_relative {
        push_module_relative_base(ctx, f); // module_off * 8
        f.instruction(&Ins::LocalGet(flat_i));
        f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
        f.instruction(&Ins::I32Mul);
        f.instruction(&Ins::I32Add);
    } else {
        f.instruction(&Ins::LocalGet(flat_i));
        f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
        f.instruction(&Ins::I32Mul);
    }
    f.instruction(&Ins::F64Load(memarg(base_byte)));
}

// ── VectorSortOrder (vm_vector_sort_order.rs:49-101) ─────────────────────────

/// Lower `VectorSortOrder { write_temp_id }`, mirroring
/// `crate::vm_vector_sort_order::vector_sort_order`. `input_view = top`; the
/// `direction` operand is popped (`.round() as i32`). The innermost (last)
/// dimension is the sorted axis; outer dims select independent rows (a scalar/1-D
/// view is one row of `inner == size`). Per row, the `(value, local_idx 0..inner)`
/// pairs are staged into the vector scratch region, sorted (ascending if
/// `direction == 1`, else descending) by the runtime [`emit_stable_sort`] helper,
/// then `temp[row_base + rank] = local_idx` is written (the 0-based in-row source
/// index at the sorted position). An invalid input view fills the temp with NaN.
pub(crate) fn emit_vector_sort_order(
    input_view: &ViewDesc,
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    // The direction operand is on the stack now; pop it to the `ascending` flag
    // first (the validity gate's body / fill_temp_nan arms must be
    // operand-balanced, so the operand is consumed before the gate). A
    // dynamically-subscripted input is handled by the gate (invalid ->
    // fill_temp_nan) and `emit_view_element_load` (runtime offset + validity).
    let ascending = ctx.vector_i32_locals[0];
    pop_direction_to_ascending(ascending, ctx, f);

    emit_with_validity_gate(&[input_view], write_temp_id, ctx, f, |ctx, f| {
        emit_vector_sort_order_body(input_view, write_temp_id, ascending, ctx, f)
    })
}

/// The valid-input body of [`emit_vector_sort_order`].
fn emit_vector_sort_order_body(
    input_view: &ViewDesc,
    write_temp_id: u8,
    ascending: u32,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let size = input_view.size();
    let n_dims = input_view.dims.len();
    let inner = if n_dims == 0 {
        size
    } else {
        input_view.dims[n_dims - 1] as usize
    };
    if inner == 0 {
        // A zero-length innermost dim yields an empty result; nothing to write.
        return Ok(());
    }

    let scratch = u64::from(ctx.vector_scratch_base);
    // Iterate rows in row-major logical order; each block of `inner` iterations
    // is one row (mirroring the VM's `increment_indices` walk -- element
    // `iter_idx` of the view, read row-major, is `flat_element_offset(iter_idx)`).
    let mut i = 0usize;
    while i < size {
        // Gather: pair[local_idx] = (value = input[i + local_idx], idx = local_idx).
        for local_idx in 0..inner {
            let pair_val_addr = scratch + (local_idx as u64) * (PAIR_BYTES as u64);
            // value slot
            f.instruction(&Ins::I32Const(0));
            emit_view_element_load(input_view, i + local_idx, ctx, f)?;
            f.instruction(&Ins::F64Store(memarg(pair_val_addr)));
            // idx slot (+8)
            f.instruction(&Ins::I32Const(0));
            f.instruction(&f64_const(local_idx as f64));
            f.instruction(&Ins::F64Store(memarg(pair_val_addr + 8)));
        }

        // stable_sort(scratch, inner, ascending)
        f.instruction(&Ins::I32Const(ctx.vector_scratch_base as i32));
        f.instruction(&Ins::I32Const(inner as i32));
        f.instruction(&Ins::LocalGet(ascending));
        f.instruction(&Ins::Call(ctx.helpers.stable_sort));

        // Scatter: temp[temp_off + i + rank] = pair[rank].idx.
        for rank in 0..inner {
            let pair_idx_addr = scratch + (rank as u64) * (PAIR_BYTES as u64) + 8;
            let temp_addr = temp_element_byte_addr(ctx, write_temp_id, (i + rank) as u32)?;
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::F64Load(memarg(pair_idx_addr)));
            f.instruction(&Ins::F64Store(memarg(temp_addr)));
        }

        i += inner;
    }
    Ok(())
}

// ── Rank (vm.rs:2540-2584) ───────────────────────────────────────────────────

/// Lower `Rank { write_temp_id }`, mirroring `vm.rs:2540-2584`. `input_view =
/// top`; the `direction` operand is popped. Over the WHOLE view, the `(value,
/// orig_idx 0..size)` pairs are staged into the vector scratch region and sorted
/// (ascending if `direction == 1`, else descending) by [`emit_stable_sort`], then
/// `temp[orig_idx] = rank_0based + 1` (1-based, indexed by original position) is
/// written. An invalid input view fills the temp with NaN.
pub(crate) fn emit_rank(
    input_view: &ViewDesc,
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let ascending = ctx.vector_i32_locals[0];
    pop_direction_to_ascending(ascending, ctx, f);

    emit_with_validity_gate(&[input_view], write_temp_id, ctx, f, |ctx, f| {
        emit_rank_body(input_view, write_temp_id, ascending, ctx, f)
    })
}

/// The valid-input body of [`emit_rank`].
fn emit_rank_body(
    input_view: &ViewDesc,
    write_temp_id: u8,
    ascending: u32,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let size = input_view.size();
    if size == 0 {
        return Ok(());
    }
    let scratch = u64::from(ctx.vector_scratch_base);
    let temp_off = *ctx
        .ctx
        .temp_offsets
        .get(write_temp_id as usize)
        .ok_or_else(|| {
            WasmGenError::Unsupported(format!("wasmgen: temp id {write_temp_id} out of range"))
        })?;

    // Gather: pair[orig_idx] = (value = input[orig_idx], idx = orig_idx).
    for orig_idx in 0..size {
        let pair_val_addr = scratch + (orig_idx as u64) * (PAIR_BYTES as u64);
        f.instruction(&Ins::I32Const(0));
        emit_view_element_load(input_view, orig_idx, ctx, f)?;
        f.instruction(&Ins::F64Store(memarg(pair_val_addr)));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&f64_const(orig_idx as f64));
        f.instruction(&Ins::F64Store(memarg(pair_val_addr + 8)));
    }

    // stable_sort(scratch, size, ascending)
    f.instruction(&Ins::I32Const(ctx.vector_scratch_base as i32));
    f.instruction(&Ins::I32Const(size as i32));
    f.instruction(&Ins::LocalGet(ascending));
    f.instruction(&Ins::Call(ctx.helpers.stable_sort));

    // Scatter by ORIGINAL position: for each rank, orig_idx = pair[rank].idx
    // (runtime); temp[temp_off + orig_idx] = rank + 1. The destination slot is
    // runtime-indexed (it depends on the sorted permutation), so the dynamic
    // address part is `orig_idx * 8` and the constant `temp_storage_base +
    // temp_off*8` rides in the `memarg.offset`. f64.store wants
    // [addr_i32, value_f64], so push the address first, then `rank + 1`.
    let temp_base_byte =
        u64::from(ctx.temp_storage_base) + (temp_off as u64) * u64::from(SLOT_SIZE);
    for rank in 0..size {
        let pair_idx_addr = scratch + (rank as u64) * (PAIR_BYTES as u64) + 8;
        // dynamic addr = orig_idx * 8, where orig_idx = trunc(pair[rank].idx).
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(pair_idx_addr)));
        f.instruction(&Ins::I32TruncSatF64S);
        f.instruction(&Ins::I32Const(SLOT_SIZE as i32));
        f.instruction(&Ins::I32Mul);
        // value = rank + 1 (1-based)
        f.instruction(&f64_const((rank + 1) as f64));
        f.instruction(&Ins::F64Store(memarg(temp_base_byte)));
    }
    Ok(())
}

/// Pop the `direction` operand off the wasm stack (the VM does `.round() as
/// i32`), compute `ascending = (round(direction) == 1) as i32`, and store it in
/// `ascending_local`. Shared by `VectorSortOrder`/`Rank`.
fn pop_direction_to_ascending(ascending_local: u32, ctx: &EmitCtx, f: &mut Function) {
    // The round's two f64 temps (`scratch_local` + `apply_locals[0]`) are both
    // free here -- nothing survives across this direction pop.
    emit_round_half_away(f, ctx.scratch_local, ctx.apply_locals[0]);
    f.instruction(&Ins::I32TruncSatF64S);
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::LocalSet(ascending_local));
}

// ── LookupArray (vm.rs:2586-2629) ────────────────────────────────────────────

/// Lower `LookupArray { base_gf, table_count, mode, write_temp_id }`, mirroring
/// `vm.rs:2586-2629`. The shared `index` is popped; `input_view = top`. For each
/// element `i`, `elem_off = flat_offset(indices)` (compile-time); if `elem_off >=
/// table_count` the result is NaN, else the GF directory entry at `base_gf +
/// elem_off` is read and the mode's Phase-3 helper (`lookup_interp`/`forward`/
/// `backward`) is `call`ed at `index`. Written to `temp[temp_off + i]` (sequential
/// index). An invalid input view fills the temp with NaN.
///
/// Each element's `elem_off` is compile-time, so the bound check, the GF
/// directory entry address, and the mode dispatch all resolve at compile time;
/// only the `index` and the `lookup_*` evaluation are runtime. Unrolled over the
/// view size (the caller charges the unroll budget).
pub(crate) fn emit_lookup_array(
    input_view: &ViewDesc,
    base_gf: GraphicalFunctionId,
    table_count: u16,
    mode: LookupMode,
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    // Pop `index` to a scratch f64 (read once per element). Done before the gate
    // so both gate arms are operand-balanced. A dynamically-subscripted input is
    // handled by the gate (invalid -> fill_temp_nan) and `emit_view_element_load`.
    let index = ctx.scratch_local;
    f.instruction(&Ins::LocalSet(index));

    emit_with_validity_gate(&[input_view], write_temp_id, ctx, f, |ctx, f| {
        emit_lookup_array_body(
            input_view,
            base_gf,
            table_count,
            mode,
            write_temp_id,
            index,
            ctx,
            f,
        )
    })
}

/// The valid-input body of [`emit_lookup_array`].
#[allow(clippy::too_many_arguments)]
fn emit_lookup_array_body(
    input_view: &ViewDesc,
    base_gf: GraphicalFunctionId,
    table_count: u16,
    mode: LookupMode,
    write_temp_id: u8,
    index: u32,
    ctx: &EmitCtx,
    f: &mut Function,
) -> Result<(), WasmGenError> {
    let helper_idx = match mode {
        LookupMode::Interpolate => ctx.helpers.lookup_interp,
        LookupMode::Forward => ctx.helpers.lookup_forward,
        LookupMode::Backward => ctx.helpers.lookup_backward,
    };
    let size = input_view.size();
    for i in 0..size {
        // elem_off (compile-time) = flat offset of element i over the view.
        let elem_off = input_view.flat_element_offset(i);
        let temp_addr = temp_element_byte_addr(ctx, write_temp_id, i as u32)?;
        f.instruction(&Ins::I32Const(0)); // temp store dynamic addr (const base)

        if elem_off >= table_count as usize {
            // Out-of-range element offset -> NaN (matching the scalar Lookup
            // bound; vm.rs:2615).
            f.instruction(&f64_const(f64::NAN));
        } else {
            // table_idx = base_gf + elem_off (compile-time). Read (data_off,
            // count) from the GF directory at gf_directory_base + table_idx*8,
            // then call the mode's helper at `index`.
            let dir_addr = u64::from(ctx.gf_directory_base)
                + (base_gf as u64 + elem_off as u64) * (GF_DIRECTORY_ENTRY_BYTES as u64);
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Load(i32_memarg(dir_addr))); // data_off
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Load(i32_memarg(dir_addr + 4))); // count
            f.instruction(&Ins::LocalGet(index));
            f.instruction(&Ins::Call(helper_idx));
        }
        f.instruction(&Ins::F64Store(memarg(temp_addr)));
    }
    Ok(())
}

// ── validity gate ────────────────────────────────────────────────────────────

/// Emit `body` for the temp-writing vector ops, gated on the VM's "`!is_valid`
/// -> fill_temp_nan" short-circuit. When no input view carries a runtime validity
/// flag (the common static/temp/full-var case), `body` is emitted directly with
/// no runtime check. Otherwise: `if all_valid { body } else { fill_temp_nan }`.
fn emit_with_validity_gate(
    views: &[&ViewDesc],
    write_temp_id: u8,
    ctx: &EmitCtx,
    f: &mut Function,
    body: impl FnOnce(&EmitCtx, &mut Function) -> Result<(), WasmGenError>,
) -> Result<(), WasmGenError> {
    let any_dynamic = views.iter().any(|v| v.valid_local.is_some());
    if !any_dynamic {
        return body(ctx, f);
    }
    push_all_valid(views, f);
    f.instruction(&Ins::If(BlockType::Empty));
    body(ctx, f)?;
    f.instruction(&Ins::Else);
    emit_fill_temp_nan(ctx, write_temp_id, f)?;
    f.instruction(&Ins::End);
    Ok(())
}
