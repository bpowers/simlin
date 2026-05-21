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
//!
//! (`VectorSortOrder`/`Rank`/`LookupArray` land in later Phase-6 tasks.)
//!
//! ## Runtime loop vs unrolled
//!
//! Each emitter here is a per-element map/gather over the *compile-time* view
//! size, so the element addresses fold into wasm constants and the bodies are
//! unrolled. The caller (`super::lower`) charges the Phase-5
//! [`EmitState`](super::lower) unroll budget for the view size before invoking
//! these, so the size cap still bounds an over-large arrayed model. (The
//! later-task stable sort backing `VectorSortOrder`/`Rank` is the exception: a
//! runtime loop, never unrolled.)
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

use super::WasmGenError;
use super::lower::{
    EmitCtx, SLOT_SIZE, emit_fill_temp_nan, emit_is_truthy, emit_view_element_load, f64_const,
    memarg, push_module_relative_base, temp_element_byte_addr,
};
use super::views::{ViewBase, ViewDesc};

/// Push `round_half_away(x)` for the f64 already on the wasm stack, reproducing
/// Rust's `f64::round` (round half AWAY from zero) -- which is what the VM uses
/// (`stack.pop().round()`, `offset_val.round()`). This is NOT wasm `f64.nearest`
/// (round half to EVEN), so the two diverge for half-integer inputs; the VM's
/// choice is reproduced via `trunc(x + copysign(0.5, x))`. For a large `x` where
/// `x + 0.5 == x` the `trunc` returns `x` unchanged, exactly as `f64::round`
/// does. `scratch` is a free f64 local used to read `x` twice.
fn emit_round_half_away(f: &mut Function, scratch: u32) {
    f.instruction(&Ins::LocalSet(scratch)); // scratch = x
    f.instruction(&Ins::LocalGet(scratch)); // x  (the addend)
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::LocalGet(scratch)); // x  (the sign source)
    f.instruction(&Ins::F64Copysign); // copysign(0.5, x): 0.5 with x's sign
    f.instruction(&Ins::F64Add); // x + copysign(0.5, x)
    f.instruction(&Ins::F64Trunc); // round half away from zero
}

// ── shared input-view helpers ───────────────────────────────────────────────

/// Whether `view` carries a runtime validity flag or runtime offset addend (a
/// dynamic subscript, Phase-5 Task 4). The vector ops handle the validity flag
/// via an op-level gate; a runtime offset addend on a *source* view is folded
/// into the runtime-indexed read.
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

    // Pop action (top) -> round-half-away -> i32; then pop max_value.
    emit_round_half_away(f, ctx.scratch_local);
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
/// subscripted input falls back to the VM.
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
            let off_indices = decompose_row_major(&offset_view.dims, i);
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
        // Compute flat_i (i32) once.
        f.instruction(&f64_const(base_i as f64));
        f.instruction(&Ins::LocalGet(offset_val));
        emit_round_half_away(f, ctx.scratch_local);
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

// ── shared geometry ──────────────────────────────────────────────────────────

/// Decompose a flat row-major iteration index into per-dimension indices (last
/// dim varies fastest), mirroring the VM's `increment_indices` walk order.
fn decompose_row_major(dims: &[u16], iter_idx: usize) -> Vec<u16> {
    let n = dims.len();
    let mut indices = vec![0u16; n];
    let mut remaining = iter_idx;
    for d in (0..n).rev() {
        let dim = dims[d] as usize;
        indices[d] = (remaining % dim) as u16;
        remaining /= dim;
    }
    indices
}
