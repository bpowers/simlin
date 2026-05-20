// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Genuine-Vensim VECTOR SORT ORDER opcode evaluation.
//!
//! Extracted from `vm.rs` (a sibling module purely for the per-file line
//! cap, like `vm_vector_elm_map`). The hot interpreter loop dispatches
//! `Opcode::VectorSortOrder` straight here.

use smallvec::SmallVec;

use crate::bytecode::{ByteCodeContext, RuntimeView, TempId};
use crate::vm::{Vm, increment_indices};

/// Genuine-Vensim VECTOR SORT ORDER.
///
/// Ranks WITHIN each currently-iterated source slice (per-row), 0-based:
/// the last-declared (innermost) dimension is the axis being sorted, and
/// every assignment of the outer dimensions selects an independent row.
/// For each row, result position `j` holds the 0-based source index --
/// *within that row* (`[0, inner)`, `inner = |last dim|`) -- of the row's
/// `j`-th element in sorted order. `direction == 1` sorts ascending,
/// otherwise descending. Ties keep stable source order (Rust's stable
/// `sort_by`); genuine Vensim leaves tie-breaking unspecified, so this does
/// not contradict it. The per-row result is therefore a valid index
/// permutation of `[0, inner)` -- a downstream `VECTOR ELM MAP` cannot read
/// out of bounds on a well-formed model.
///
/// Results are written to `temp_storage` in the view's row-major logical
/// order (the last dim varies fastest), so a contiguous block of `inner`
/// slots is exactly one row; reads use `flat_offset` so physically strided
/// or sparse source views are handled correctly.
///
/// A 1-D (effectively single-row) view is the degenerate case where the
/// row IS the whole view, so its in-row ranks equal the whole-view ranks --
/// byte-identical to the AC5 / Phase 4 single-row 0-based output it
/// generalizes. (GH #585: the prior implementation ranked over the whole
/// flattened view and emitted absolute flat indices, so for a multi-row
/// `[COP,Target]` source the non-first rows got global offsets like
/// `[18,19,20]` instead of the in-row ranks `[0,1,2]`, which then indexed a
/// single-column ELM MAP source out of bounds.)
///
/// Ground truth for the 0-based (not 1-based) convention: real Vensim DSS
/// 7.3.4 reference output `test/test-models/tests/vector_order/output.tab`
/// (`SORT ORDER[*]` ranges over `0..n-1` and contains `0`, impossible for a
/// 1-based permutation) and Ventana's official VECTOR SORT ORDER reference.
/// (RANK is a distinct, correctly 1-based opcode, per the same file.)
pub(crate) fn vector_sort_order(
    input_view: &RuntimeView,
    direction: i32,
    write_temp_id: TempId,
    curr: &[f64],
    temp_storage: &mut [f64],
    context: &ByteCodeContext,
) {
    if !input_view.is_valid {
        Vm::fill_temp_nan(temp_storage, context, write_temp_id);
        return;
    }

    let size = input_view.size();
    let n_dims = input_view.dims.len();
    // The innermost (last-declared) dimension is the sorted axis; outer dims
    // select independent rows. A scalar/empty view (n_dims == 0) has no axis
    // to sort and `size == 1` would write a single 0; treat the whole view as
    // one row in that degenerate case so `inner == size`.
    let inner = if n_dims == 0 {
        size
    } else {
        input_view.dims[n_dims - 1] as usize
    };

    let temp_off = context.temp_offsets[write_temp_id as usize];

    // Iterate in row-major logical order (last dim fastest). Each contiguous
    // block of `inner` iterations is one row; rank within the block and write
    // 0-based in-row source positions back to the same block's temp slots.
    let mut indices: SmallVec<[u16; 4]> = smallvec::smallvec![0; n_dims];
    let mut row: SmallVec<[(f64, usize); 32]> = SmallVec::with_capacity(inner.max(1));
    let mut i = 0usize;
    while i < size {
        row.clear();
        for local_idx in 0..inner {
            let flat_off = input_view.flat_offset(&indices);
            let val = Vm::read_view_element(input_view, flat_off, curr, temp_storage, context);
            row.push((val, local_idx));
            increment_indices(&mut indices, &input_view.dims);
        }

        if direction == 1 {
            row.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            row.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        }

        for (rank, &(_, local_idx)) in row.iter().enumerate() {
            temp_storage[temp_off + i + rank] = local_idx as f64;
        }
        i += inner;
    }
}
