// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Genuine-Vensim VECTOR ELM MAP opcode evaluation.
//!
//! Extracted from `vm.rs` (a sibling module purely for the per-file line
//! cap, like other `vm`-adjacent helpers in this crate). The hot
//! interpreter loop dispatches `Opcode::VectorElmMap` straight here.

use smallvec::SmallVec;

use crate::bytecode::{ByteCodeContext, RuntimeView, TempId};
use crate::vm::{Vm, increment_indices};

/// Genuine-Vensim VECTOR ELM MAP: result element `i` =
/// `source[base_i + round(offset[i])]` over the source variable's FULL
/// row-major contiguous storage. `base_i` is the flat position the
/// first-argument *element reference* establishes; the offset steps the
/// source's innermost (last declared) dimension (stride 1 in contiguous
/// storage). An offset+base outside `[0, full_source_len)`, or a NaN
/// offset, yields `:NA:` (NaN). NO modulo / NO wraparound (the prior
/// implementation flattened the *sliced* view with no per-element base --
/// the bug this corrects).
///
/// Citations: Ventana Systems' official Vensim VECTOR ELM MAP reference
/// (0-based offset; base from arg-1's element reference; out-of-range =>
/// :NA:, no modulo; full array, last subscript fastest) and real-Vensim
/// ground truth `test/sdeverywhere/models/vector/vector.dat` (identical
/// `vector_simple` rows): `c=[11,12,12]`, `f=[1,5,6]` (broadcast across
/// DimB), `g=[1,4,5,2,3,6]`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn vector_elm_map(
    source_view: &RuntimeView,
    offset_view: &RuntimeView,
    write_temp_id: TempId,
    full_source_len: u32,
    curr: &[f64],
    temp_storage: &mut [f64],
    context: &ByteCodeContext,
) {
    if !source_view.is_valid || !offset_view.is_valid {
        Vm::fill_temp_nan(temp_storage, context, write_temp_id);
        return;
    }
    let temp_off = context.temp_offsets[write_temp_id as usize];
    let full_len = full_source_len as usize;

    // Full contiguous source => no per-element base (offset indexes the
    // whole array). Otherwise a strict slice: remaining dims are the
    // carried axes, `offset` holds the collapsed subscript's base.
    let source_is_full_array = source_view.size() == full_len && source_view.is_contiguous();

    // Carried source dim -> offset view axis of the same dimension id (so
    // the sliced view's `flat_offset` evaluates the element reference at
    // this result element); uncarried axes contribute 0.
    let src_to_off_axis: SmallVec<[Option<usize>; 4]> = source_view
        .dim_ids
        .iter()
        .map(|sd| offset_view.dim_ids.iter().position(|od| od == sd))
        .collect();

    // Strict-slice carried-axis invariant: when the source is NOT a full
    // contiguous array every remaining (carried) source axis must appear in
    // the offset view's dim_ids, so its `src_indices` slot is driven by the
    // result element (the `None => 0` arm below is only the structurally
    // unreachable uncarried-axis fallback for valid lowered shapes -- all
    // exercised shapes, 1-D promoted, cross-dim `d[DimA,B1]`, and
    // scalar-broadcast, satisfy this). An unresolved carried axis would
    // silently read element 0 of that dimension (the silent-wrong
    // direction), so make it loud in debug builds. Full-array sources skip
    // this: `base_i` is hard-coded to 0 there and `src_to_off_axis` is
    // unused.
    debug_assert!(
        source_is_full_array || src_to_off_axis.iter().all(Option::is_some),
        "VECTOR ELM MAP strict-slice: every carried source axis must appear in the offset view's dim_ids; an unresolved carried axis would silently read element 0"
    );

    let offset_size = offset_view.size();
    let mut off_indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; offset_view.dims.len()];
    let mut src_indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; source_view.dims.len()];
    for i in 0..offset_size {
        let off_flat = offset_view.flat_offset(&off_indices);
        let offset_val = Vm::read_view_element(offset_view, off_flat, curr, temp_storage, context);

        // base_i: 0 for a full-array source; else the sliced view's flat
        // offset at this element's carried-dim projection.
        let base_i = if source_is_full_array {
            0i64
        } else {
            for (k, slot) in src_indices.iter_mut().enumerate() {
                *slot = match src_to_off_axis[k] {
                    Some(p) => off_indices[p],
                    None => 0,
                };
            }
            source_view.flat_offset(&src_indices) as i64
        };

        // Contiguous row-major storage: stepping the source's innermost
        // (last declared) dimension is a step of 1 flat slot. Read via
        // read_view_element (strict-slice views may be temp-backed).
        let elem = if offset_val.is_nan() {
            f64::NAN
        } else {
            let flat_i = base_i + offset_val.round() as i64;
            if flat_i < 0 || flat_i >= full_len as i64 {
                f64::NAN
            } else {
                Vm::read_view_element(source_view, flat_i as usize, curr, temp_storage, context)
            }
        };
        temp_storage[temp_off + i] = elem;
        increment_indices(&mut off_indices, &offset_view.dims);
    }
}
