// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure compile-time model of the VM's runtime `view_stack`. No I/O; the only
// state is the `Vec<ViewDesc>` the emitter threads through `emit_bytecode`.

//! Compile-time view descriptors -- the wasm backend's analogue of the VM's
//! runtime `view_stack` (`crate::vm`).
//!
//! The VM resolves every array access through a runtime stack of [`RuntimeView`]s
//! built and transformed by the `Push*View` / `View*` opcodes. Because every
//! static view's geometry (base offset, dims, strides, offset, sparsity,
//! is_temp) is known at compile time, the wasm emitter maintains a *compile-time*
//! stack of [`ViewDesc`]s instead, mirroring the static parts of `RuntimeView`
//! field-for-field and reproducing the `RuntimeView::apply_*` transforms in
//! `apply_*` here. Element addressing then routes through a single source of
//! truth -- [`ViewDesc::element_addr`] -- so Tasks 2-4 and Phase 6 all address
//! elements identically to the VM's `flat_offset` / `offset_for_iter_index`.
//!
//! [`RuntimeView`]: crate::bytecode::RuntimeView

use crate::bytecode::{ByteCodeContext, StaticArrayView};

/// Where a view's base address lives, mirroring how the VM resolves the base of
/// a `RuntimeView` element read (`reduce_view` in `vm.rs`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewBase {
    /// `curr[base_off + ..]` at an *absolute* slot base. This is what
    /// `PushStaticView` produces: `StaticArrayView::to_runtime_view` copies
    /// `base_off` verbatim (no `module_off` added), so the byte address is
    /// `curr_base + (base_off + flat) * 8` with no runtime addend.
    CurrAbsolute,
    /// `curr[module_off + base_off + ..]`. `PushVarView` / `PushVarViewDirect`
    /// fold the runtime `module_off` into the base (`vm.rs:1749` / `1784`), so a
    /// read adds `module_off * 8` to the constant address. In the current
    /// single-root scope `module_off == 0`, but the distinction is preserved so
    /// Phase 7 can thread a real `module_off` without changing addressing.
    CurrModuleRelative,
    /// `temp_storage[temp_offsets[base_off] + ..]` (`is_temp`): the base is a
    /// temp id, resolved against the `temp_storage` region via `temp_offsets`.
    Temp,
}

/// A single sparse-dimension mapping, mirroring
/// [`crate::bytecode::RuntimeSparseMapping`]: the view's index along
/// `dim_index` is remapped through `parent_offsets` before being multiplied by
/// the stride (`RuntimeView::flat_offset`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct SparseDim {
    pub dim_index: usize,
    pub parent_offsets: Vec<u16>,
}

/// Compile-time mirror of the static parts of [`crate::bytecode::RuntimeView`].
///
/// Holds exactly the geometry needed to compute an element's byte address:
/// `base` (where the storage lives), `dims`/`strides`/`offset`/`sparse` (the
/// flat-offset arithmetic), and `dim_ids` (broadcast matching, used by Phase 5
/// Task 3's iteration). `runtime_off_local` / `valid_local` are `None` for every
/// static view; Task 4's dynamic subscripts set them to wasm locals carrying a
/// runtime offset addend and a validity flag.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct ViewDesc {
    /// Base slot offset (in `curr`) or temp id (when `base == Temp`).
    pub base_off: u32,
    pub base: ViewBase,
    /// Dimension sizes (`size() == product`).
    pub dims: Vec<u16>,
    /// Per-dimension strides (signed: a transposed view has non-row-major,
    /// still-positive strides; the sign supports future reversed views).
    pub strides: Vec<i32>,
    /// Starting flat offset within the base array (folds in collapsed subscripts
    /// and range starts).
    pub offset: u32,
    /// Sparse dimension mappings (empty unless a star-range was applied).
    pub sparse: Vec<SparseDim>,
    /// Dimension IDs, for broadcast matching during iteration (Task 3).
    pub dim_ids: Vec<u16>,
    /// wasm i32 local holding a runtime offset addend (dynamic subscript, Task
    /// 4). `None` for static views.
    pub runtime_off_local: Option<u32>,
    /// wasm i32 local that is 0 when the view is invalid (out-of-bounds dynamic
    /// subscript, Task 4). `None` for static views (always valid).
    pub valid_local: Option<u32>,
}

impl ViewDesc {
    /// Build a `ViewDesc` from a baked [`StaticArrayView`] (`PushStaticView`).
    ///
    /// `StaticArrayView::to_runtime_view` copies `base_off` verbatim with no
    /// `module_off`, so the base is [`ViewBase::CurrAbsolute`] for a variable
    /// view and [`ViewBase::Temp`] when `is_temp`.
    pub fn from_static(view: &StaticArrayView) -> Self {
        ViewDesc {
            base_off: view.base_off,
            base: if view.is_temp {
                ViewBase::Temp
            } else {
                ViewBase::CurrAbsolute
            },
            dims: view.dims.to_vec(),
            strides: view.strides.to_vec(),
            offset: view.offset,
            sparse: view
                .sparse
                .iter()
                .map(|s| SparseDim {
                    dim_index: s.dim_index as usize,
                    parent_offsets: s.parent_offsets.to_vec(),
                })
                .collect(),
            dim_ids: view.dim_ids.to_vec(),
            runtime_off_local: None,
            valid_local: None,
        }
    }

    /// Build a contiguous view over a full variable/temp array from a dim-list
    /// (the `(n_dims, sizes)` for `PushVarViewDirect`, or dim sizes resolved
    /// from `ctx.dimensions` for `PushVarView`/`PushTempView`). Strides are
    /// row-major, built right-to-left, exactly as `RuntimeView::for_var`.
    pub fn contiguous(base_off: u32, base: ViewBase, dims: Vec<u16>, dim_ids: Vec<u16>) -> Self {
        let mut strides = Vec::with_capacity(dims.len());
        let mut stride = 1i32;
        for &d in dims.iter().rev() {
            strides.push(stride);
            stride *= d as i32;
        }
        strides.reverse();
        ViewDesc {
            base_off,
            base,
            dims,
            strides,
            offset: 0,
            sparse: Vec::new(),
            dim_ids,
            runtime_off_local: None,
            valid_local: None,
        }
    }

    /// `size() == product of dims` (`RuntimeView::size`). A scalar view (no
    /// dims) has size 1. The array reducer (Task 2) bounds its unrolled fold by
    /// this.
    pub fn size(&self) -> usize {
        self.dims.iter().map(|&d| d as usize).product()
    }

    /// Whether the view is contiguous: offset 0, no sparse mappings, and
    /// row-major strides (`RuntimeView::is_contiguous`).
    pub fn is_contiguous(&self) -> bool {
        if self.offset != 0 || !self.sparse.is_empty() {
            return false;
        }
        let mut expected = 1i32;
        for i in (0..self.dims.len()).rev() {
            if self.strides[i] != expected {
                return false;
            }
            expected *= self.dims[i] as i32;
        }
        true
    }

    /// Apply a single-element subscript at `dim_idx` (0-based index), dropping
    /// that dimension. Exactly mirrors `RuntimeView::apply_single_subscript`:
    /// a sparse dim's index is first remapped through `parent_offsets` (and the
    /// mapping removed), the resolved index is folded into `offset`, the
    /// dimension is removed, and later sparse mappings shift down by one.
    pub fn apply_single_subscript(&mut self, dim_idx: usize, index: u16) {
        let actual_index =
            if let Some(pos) = self.sparse.iter().position(|s| s.dim_index == dim_idx) {
                let parent_idx = self.sparse[pos].parent_offsets[index as usize];
                self.sparse.remove(pos);
                parent_idx
            } else {
                index
            };

        self.offset += actual_index as u32 * self.strides[dim_idx] as u32;

        self.dims.remove(dim_idx);
        self.strides.remove(dim_idx);
        self.dim_ids.remove(dim_idx);

        for s in &mut self.sparse {
            if s.dim_index > dim_idx {
                s.dim_index -= 1;
            }
        }
    }

    /// Apply a `[start:end)` range (0-based) to `dim_idx`
    /// (`RuntimeView::apply_range`): fold the start into `offset` and shrink the
    /// dimension to `end - start`.
    pub fn apply_range(&mut self, dim_idx: usize, start: u16, end: u16) {
        self.offset += start as u32 * self.strides[dim_idx] as u32;
        self.dims[dim_idx] = end - start;
    }

    /// Apply a star-range (sparse) at `dim_idx`
    /// (`RuntimeView::apply_sparse_with_dim_id`): the dimension's size becomes
    /// the number of parent offsets, a sparse mapping is recorded, and the
    /// dim id is relabeled to the subdimension for broadcast matching.
    pub fn apply_sparse(&mut self, dim_idx: usize, parent_offsets: Vec<u16>, new_dim_id: u16) {
        self.dims[dim_idx] = parent_offsets.len() as u16;
        self.sparse.push(SparseDim {
            dim_index: dim_idx,
            parent_offsets,
        });
        self.dim_ids[dim_idx] = new_dim_id;
    }

    /// Transpose the view (`RuntimeView::transpose`): reverse dims/strides/
    /// dim_ids and renumber the sparse `dim_index`es to `n-1-dim_index`.
    pub fn transpose(&mut self) {
        self.dims.reverse();
        self.strides.reverse();
        self.dim_ids.reverse();
        let n = self.dims.len();
        for s in &mut self.sparse {
            s.dim_index = n - 1 - s.dim_index;
        }
    }

    /// The flat element offset (within the base array, in slots) for a flat
    /// iteration index `iter_idx in 0..size()`. Mirrors
    /// `RuntimeView::offset_for_iter_index` + `flat_offset`: contiguous views
    /// short-circuit to `offset + iter_idx`; otherwise the flat index is
    /// decomposed into row-major multi-dim indices and each (sparse-remapped)
    /// index multiplied by its stride.
    pub fn flat_element_offset(&self, iter_idx: usize) -> usize {
        if self.dims.is_empty() {
            return self.offset as usize;
        }
        if self.is_contiguous() {
            return self.offset as usize + iter_idx;
        }

        // Decompose iter_idx into per-dimension indices (last dim varies fastest).
        let n = self.dims.len();
        let mut indices = vec![0u16; n];
        let mut remaining = iter_idx;
        for d in (0..n).rev() {
            let dim = self.dims[d] as usize;
            indices[d] = (remaining % dim) as u16;
            remaining /= dim;
        }

        let mut flat = self.offset as usize;
        for (i, &idx) in indices.iter().enumerate() {
            let actual = if let Some(s) = self.sparse.iter().find(|s| s.dim_index == i) {
                s.parent_offsets[idx as usize] as usize
            } else {
                idx as usize
            };
            flat += actual * self.strides[i] as usize;
        }
        flat
    }

    /// The flat element offset (in slots) for an explicit multi-dimensional
    /// index, mirroring `RuntimeView::flat_offset`: `offset + Σ idx_k *
    /// strides[k]`, with a sparse dimension's index first remapped through its
    /// `parent_offsets`. The broadcast paths below build the multi-dim index
    /// themselves (rather than from a flat iteration index), so they route
    /// through this rather than [`flat_element_offset`](Self::flat_element_offset).
    pub fn flat_offset_for_indices(&self, indices: &[u16]) -> usize {
        let mut flat = self.offset as usize;
        for (i, &idx) in indices.iter().enumerate() {
            let actual = if let Some(s) = self.sparse.iter().find(|s| s.dim_index == i) {
                s.parent_offsets[idx as usize] as usize
            } else {
                idx as usize
            };
            flat += actual * self.strides[i] as usize;
        }
        flat
    }

    /// Decompose a flat iteration index into per-dimension indices in row-major
    /// order (last dim varies fastest), mirroring the VM's iteration-index
    /// decomposition in `LoadIterViewTop` / `reduce_view` / `increment_indices`.
    fn decompose_iter_index(dims: &[u16], iter_idx: usize) -> Vec<u16> {
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

    /// The flat element offset (in slots) for reading `self` as the *source* of
    /// an iteration whose output geometry is `iter` at flat index `current`,
    /// reproducing the VM's `LoadIterViewTop` / `LoadIterViewAt` broadcast
    /// (`vm.rs:1946-2182`). Returns `None` when the VM would push NaN: a smaller
    /// source than the iteration, or a dimension that does not match.
    ///
    /// Fast path (source dims/dim_ids equal the iteration's): the simple
    /// `offset_for_iter_index(current)` read, bounds-checked against the source
    /// size. Otherwise the broadcast path decomposes `current` into the
    /// iteration's multi-dim indices, matches dimensions through
    /// [`crate::dimensions::match_dimensions_two_pass`] (exact dim-id match, then
    /// the indexed size-fallback), and rebuilds the source indices (bounds-checked
    /// per dimension). `is_indexed` for each dim comes from `ctx.dimensions`,
    /// exactly as the VM resolves it.
    pub fn iter_broadcast_offset(
        &self,
        iter: &ViewDesc,
        current: usize,
        ctx: &ByteCodeContext,
    ) -> Option<usize> {
        // Fast path: dims and dim_ids match exactly -> direct iteration-index read
        // (with the VM's "source smaller than iteration -> NaN" bounds check).
        if self.dims == iter.dims && self.dim_ids == iter.dim_ids {
            if current >= self.size() {
                return None;
            }
            return Some(self.flat_element_offset(current));
        }

        // Broadcast path: decompose `current` into the iteration's indices, then
        // map each source dimension to an iteration dimension.
        let iter_indices = Self::decompose_iter_index(&iter.dims, current);

        let dim_indexed = |dim_ids: &[u16]| -> Vec<bool> {
            dim_ids
                .iter()
                .map(|&dim_id| {
                    ctx.dimensions
                        .get(dim_id as usize)
                        .is_some_and(|d| d.is_indexed)
                })
                .collect()
        };
        let source_is_indexed = dim_indexed(&self.dim_ids);
        let iter_is_indexed = dim_indexed(&iter.dim_ids);

        let source_to_iter = crate::dimensions::match_dimensions_two_pass(
            &self.dim_ids,
            &self.dims,
            &source_is_indexed,
            &iter.dim_ids,
            &iter.dims,
            &iter_is_indexed,
        );

        let mut source_indices: Vec<u16> = Vec::with_capacity(self.dims.len());
        for (src_dim_pos, mapped_iter_pos) in source_to_iter.iter().enumerate() {
            let iter_pos = (*mapped_iter_pos)?;
            let idx = iter_indices[iter_pos];
            if idx >= self.dims[src_dim_pos] {
                return None;
            }
            source_indices.push(idx);
        }
        Some(self.flat_offset_for_indices(&source_indices))
    }

    /// The byte address of view element `iter_idx`, decomposed into the constant
    /// part (which rides in a `memarg.offset`) and whether a runtime `module_off`
    /// addend is still required. This is the single source of truth for element
    /// addressing -- the unrolled reducer (Task 2), the iteration loop (Task 3),
    /// and Phase 6 all route through it.
    ///
    /// - `CurrAbsolute`: `const = curr_base + (base_off + flat) * 8`,
    ///   `module_relative = false` (static views bake `module_off` in already).
    /// - `Temp`: `const = temp_storage_base + (temp_offsets[base_off] + flat)*8`,
    ///   `module_relative = false`.
    /// - `CurrModuleRelative`: `const = curr_base + (base_off + flat) * 8`,
    ///   `module_relative = true` (the caller adds `module_off * 8`). The VM
    ///   folds `module_off` into the base at `PushVarView` time (`vm.rs:1749`);
    ///   in the single-root scope `module_off == 0`, so the read is the same as
    ///   `CurrAbsolute` today, but the flag keeps Phase 7 correct.
    ///
    /// Returns `None` for a dynamically-subscripted view (`runtime_off_local`
    /// set, Task 4) -- those need an extra runtime addend the const form cannot
    /// express.
    pub fn element_addr(
        &self,
        iter_idx: usize,
        curr_base: u32,
        temp_storage_base: u32,
        ctx: &ByteCodeContext,
    ) -> Option<ElementAddr> {
        let flat = self.flat_element_offset(iter_idx);
        self.element_addr_for_flat(flat, curr_base, temp_storage_base, ctx)
    }

    /// Like [`element_addr`](Self::element_addr) but for an *already-computed*
    /// flat slot offset (the broadcast paths build the flat offset themselves via
    /// [`flat_offset_for_indices`](Self::flat_offset_for_indices), rather than
    /// from an iteration index). Static-view behaviour is byte-identical to
    /// `element_addr` for the same flat offset.
    pub fn element_addr_for_flat(
        &self,
        flat: usize,
        curr_base: u32,
        temp_storage_base: u32,
        ctx: &ByteCodeContext,
    ) -> Option<ElementAddr> {
        if self.runtime_off_local.is_some() {
            return None;
        }
        let flat = flat as u64;
        match self.base {
            ViewBase::CurrAbsolute => Some(ElementAddr {
                const_byte_offset: u64::from(curr_base) + (u64::from(self.base_off) + flat) * 8,
                module_relative: false,
            }),
            ViewBase::CurrModuleRelative => Some(ElementAddr {
                const_byte_offset: u64::from(curr_base) + (u64::from(self.base_off) + flat) * 8,
                module_relative: true,
            }),
            ViewBase::Temp => {
                let temp_off = *ctx.temp_offsets.get(self.base_off as usize)? as u64;
                Some(ElementAddr {
                    const_byte_offset: u64::from(temp_storage_base) + (temp_off + flat) * 8,
                    module_relative: false,
                })
            }
        }
    }
}

/// The byte address of a view element, split into the compile-time-constant
/// part (a `memarg.offset`) and whether the emitter must still add a runtime
/// `module_off * 8`. Returned by [`ViewDesc::element_addr`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ElementAddr {
    pub const_byte_offset: u64,
    pub module_relative: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{RuntimeSparseMapping, RuntimeView};
    use smallvec::SmallVec;

    /// Build the VM `RuntimeView` equivalent of a `ViewDesc` so the two
    /// addressing implementations can be cross-checked. Validity/runtime locals
    /// are not part of the geometry, so a static-shaped `ViewDesc` maps directly.
    fn to_runtime_view(d: &ViewDesc) -> RuntimeView {
        RuntimeView {
            base_off: d.base_off,
            is_temp: matches!(d.base, ViewBase::Temp),
            dims: SmallVec::from_slice(&d.dims),
            strides: SmallVec::from_slice(&d.strides),
            offset: d.offset,
            sparse: d
                .sparse
                .iter()
                .map(|s| RuntimeSparseMapping {
                    dim_index: s.dim_index as u8,
                    parent_offsets: SmallVec::from_slice(&s.parent_offsets),
                })
                .collect(),
            dim_ids: SmallVec::from_slice(&d.dim_ids),
            is_valid: true,
        }
    }

    /// Assert `ViewDesc::flat_element_offset` agrees with the VM's
    /// `RuntimeView::offset_for_iter_index` for every element of the view -- the
    /// addressing oracle Task 1 must match.
    fn assert_flat_matches_vm(d: &ViewDesc) {
        let rv = to_runtime_view(d);
        assert_eq!(d.size(), rv.size(), "size mismatch");
        assert_eq!(d.is_contiguous(), rv.is_contiguous(), "contiguity mismatch");
        for i in 0..d.size() {
            assert_eq!(
                d.flat_element_offset(i),
                rv.offset_for_iter_index(i),
                "flat offset mismatch at element {i}"
            );
        }
    }

    fn dense(base_off: u32, dims: &[u16]) -> ViewDesc {
        ViewDesc::contiguous(
            base_off,
            ViewBase::CurrAbsolute,
            dims.to_vec(),
            vec![0u16; dims.len()],
        )
    }

    #[test]
    fn contiguous_1d_addresses_match_vm() {
        assert_flat_matches_vm(&dense(0, &[5]));
        assert_flat_matches_vm(&dense(7, &[5]));
    }

    #[test]
    fn contiguous_2d_addresses_match_vm() {
        assert_flat_matches_vm(&dense(0, &[2, 3]));
        assert_flat_matches_vm(&dense(0, &[3, 4]));
    }

    #[test]
    fn subscript_const_drops_dim_like_vm() {
        // 2x3 matrix; subscript dim 0 to index 1 -> a 1-D row at offset 3.
        let mut d = dense(0, &[2, 3]);
        let mut rv = to_runtime_view(&d);
        d.apply_single_subscript(0, 1);
        rv.apply_single_subscript(0, 1);
        assert_eq!(d.offset, rv.offset);
        assert_eq!(d.dims.as_slice(), rv.dims.as_slice());
        assert_eq!(d.strides.as_slice(), rv.strides.as_slice());
        assert_flat_matches_vm(&d);
    }

    #[test]
    fn range_matches_vm() {
        // [1:4) of a 5-element dim: offset 1, dim 3.
        let mut d = dense(0, &[5]);
        d.apply_range(0, 1, 4);
        assert_eq!(d.offset, 1);
        assert_eq!(d.dims, vec![3]);
        assert_flat_matches_vm(&d);
    }

    #[test]
    fn transpose_matches_vm() {
        let mut d = dense(0, &[2, 3]);
        let mut rv = to_runtime_view(&d);
        d.transpose();
        rv.transpose();
        assert_eq!(d.dims.as_slice(), rv.dims.as_slice());
        assert_eq!(d.strides.as_slice(), rv.strides.as_slice());
        assert!(
            !d.is_contiguous(),
            "a transposed 2x3 view is non-contiguous"
        );
        assert_flat_matches_vm(&d);
    }

    #[test]
    fn star_range_sparse_matches_vm() {
        // A 1-D dim of 4, star-ranged to parent offsets [1, 3].
        let mut d = dense(0, &[4]);
        let mut rv = to_runtime_view(&d);
        d.apply_sparse(0, vec![1, 3], 1);
        rv.apply_sparse_with_dim_id(0, SmallVec::from_slice(&[1, 3]), 1);
        assert_eq!(d.dims, vec![2]);
        assert_flat_matches_vm(&d);
        // The two selected elements map to parent flat offsets 1 and 3.
        assert_eq!(d.flat_element_offset(0), 1);
        assert_eq!(d.flat_element_offset(1), 3);
    }

    #[test]
    fn subscript_then_renumbers_sparse_like_vm() {
        // A 2-D view [3,4] with a sparse mapping on dim 1; subscript dim 0 must
        // shift the sparse dim_index down to 0, matching the VM.
        let mut d = dense(0, &[3, 4]);
        d.apply_sparse(1, vec![0, 2], 5); // sparse on dim 1 -> dim 1 size 2
        let mut rv = to_runtime_view(&d);
        d.apply_single_subscript(0, 1);
        rv.apply_single_subscript(0, 1);
        assert_eq!(d.sparse.len(), 1);
        assert_eq!(d.sparse[0].dim_index, rv.sparse[0].dim_index as usize);
        assert_flat_matches_vm(&d);
    }

    #[test]
    fn element_addr_curr_absolute_const() {
        let d = dense(2, &[3]);
        let ctx = ByteCodeContext::default();
        // element 1 at curr_base=0: (base_off 2 + flat 1) * 8 = 24.
        let a = d.element_addr(1, 0, 0, &ctx).unwrap();
        assert_eq!(a.const_byte_offset, 24);
        assert!(!a.module_relative);
    }

    #[test]
    fn element_addr_curr_module_relative_flag() {
        let d = ViewDesc::contiguous(2, ViewBase::CurrModuleRelative, vec![3], vec![0]);
        let ctx = ByteCodeContext::default();
        let a = d.element_addr(1, 0, 0, &ctx).unwrap();
        assert_eq!(a.const_byte_offset, 24);
        assert!(
            a.module_relative,
            "var views carry a runtime module_off addend"
        );
    }

    #[test]
    fn element_addr_temp_uses_offset_table() {
        let mut ctx = ByteCodeContext::default();
        ctx.set_temp_info(vec![0, 4], 8);
        let d = ViewDesc::contiguous(1, ViewBase::Temp, vec![2], vec![0]);
        // temp_storage_base = 1000; temp 1 offset = 4; element 1 -> (4+1)*8 = 40.
        let a = d.element_addr(1, 0, 1000, &ctx).unwrap();
        assert_eq!(a.const_byte_offset, 1000 + 40);
        assert!(!a.module_relative);
    }

    #[test]
    fn element_addr_dynamic_view_is_none() {
        // A view with a runtime offset addend (Task 4) cannot be addressed by the
        // const form.
        let mut d = dense(0, &[3]);
        d.runtime_off_local = Some(9);
        let ctx = ByteCodeContext::default();
        assert!(d.element_addr(0, 0, 0, &ctx).is_none());
    }

    // ── iter_broadcast_offset (Task 3): cross-check against the VM ─────────

    /// A `ByteCodeContext` whose dimension table makes the dims with the given
    /// ids indexed (so `match_dimensions_two_pass`'s size-fallback can fire), all
    /// of `size`. Used only so `iter_broadcast_offset` can resolve `is_indexed`.
    fn ctx_indexed_dims(n: usize, size: u16) -> ByteCodeContext {
        let mut ctx = ByteCodeContext::default();
        for _ in 0..n {
            let nid = ctx.intern_name("D");
            ctx.add_dimension(crate::bytecode::DimensionInfo::indexed(nid, size));
        }
        ctx
    }

    /// Build a `ViewDesc` with explicit dims/dim_ids (row-major contiguous).
    fn view_with_dim_ids(dims: &[u16], dim_ids: &[u16]) -> ViewDesc {
        ViewDesc::contiguous(0, ViewBase::CurrAbsolute, dims.to_vec(), dim_ids.to_vec())
    }

    #[test]
    fn iter_broadcast_offset_matches_fast_path() {
        // Source dims == iter dims: every element reads its own offset.
        let ctx = ctx_indexed_dims(2, 3);
        let iter = view_with_dim_ids(&[2, 3], &[0, 1]);
        let src = view_with_dim_ids(&[2, 3], &[0, 1]);
        for current in 0..iter.size() {
            assert_eq!(
                src.iter_broadcast_offset(&iter, current, &ctx),
                Some(current),
                "fast-path element {current}"
            );
        }
    }

    #[test]
    fn iter_broadcast_offset_broadcasts_smaller_source() {
        // iter is 2-D [DimA(2), DimB(3)]; source is 1-D [DimA(2)] (dim_id 0). The
        // VM broadcasts the source along the missing DimB, so result element
        // (a, b) reads source[a]. dim_ids: iter [0,1], source [0].
        let ctx = ctx_indexed_dims(2, 3);
        let iter = view_with_dim_ids(&[2, 3], &[0, 1]);
        let src = view_with_dim_ids(&[2], &[0]);
        for a in 0..2u16 {
            for b in 0..3u16 {
                let current = (a as usize) * 3 + b as usize;
                // Result element (a,b) -> source index [a] -> flat offset a.
                assert_eq!(
                    src.iter_broadcast_offset(&iter, current, &ctx),
                    Some(a as usize),
                    "broadcast element ({a},{b})"
                );
            }
        }
    }

    #[test]
    fn iter_broadcast_offset_smaller_source_same_shape_is_nan() {
        // Same dims/dim_ids fast path, but the source is genuinely shorter than
        // the iteration: the VM returns NaN past the source size.
        let ctx = ctx_indexed_dims(1, 5);
        let iter = view_with_dim_ids(&[5], &[0]);
        let src = view_with_dim_ids(&[3], &[0]);
        assert_eq!(src.iter_broadcast_offset(&iter, 2, &ctx), Some(2));
        assert_eq!(
            src.iter_broadcast_offset(&iter, 3, &ctx),
            None,
            "element past the source size must be NaN"
        );
    }

    #[test]
    fn iter_broadcast_offset_unmatched_dim_is_nan() {
        // Source dim_id 7 has no counterpart in the iteration (dim_ids [0,1]) and
        // is named (not indexed), so the size-fallback cannot match it either:
        // the VM returns NaN.
        let mut ctx = ByteCodeContext::default();
        let n0 = ctx.intern_name("A");
        ctx.add_dimension(crate::bytecode::DimensionInfo::indexed(n0, 2)); // id 0
        let n1 = ctx.intern_name("B");
        ctx.add_dimension(crate::bytecode::DimensionInfo::indexed(n1, 3)); // id 1
        // A named (non-indexed) dim id 2 used only by the source.
        let n2 = ctx.intern_name("C");
        ctx.add_dimension(crate::bytecode::DimensionInfo::named(
            n2,
            SmallVec::from_slice(&[n0, n1]),
        )); // id 2, size 2, named
        let iter = view_with_dim_ids(&[2, 3], &[0, 1]);
        let src = view_with_dim_ids(&[2], &[2]);
        assert_eq!(src.iter_broadcast_offset(&iter, 0, &ctx), None);
    }

    /// Cross-check `iter_broadcast_offset` against a from-scratch reimplementation
    /// of the VM's `LoadIterViewTop` broadcast over a `RuntimeView`, for a
    /// transpose-broadcast case (iter [DimA,DimB], source [DimB] -- the source's
    /// single dim matches the iteration's *second* axis by dim-id).
    #[test]
    fn iter_broadcast_offset_matches_vm_loaditerviewtop() {
        let ctx = ctx_indexed_dims(2, 0); // sizes overwritten below
        // Rebuild with distinct sizes: DimA=2 (id 0), DimB=4 (id 1).
        let mut ctx2 = ByteCodeContext::default();
        let na = ctx2.intern_name("A");
        ctx2.add_dimension(crate::bytecode::DimensionInfo::indexed(na, 2));
        let nb = ctx2.intern_name("B");
        ctx2.add_dimension(crate::bytecode::DimensionInfo::indexed(nb, 4));
        let _ = ctx;

        let iter = view_with_dim_ids(&[2, 4], &[0, 1]);
        let src = view_with_dim_ids(&[4], &[1]); // only DimB
        let iter_rv = to_runtime_view(&iter);
        let src_rv = to_runtime_view(&src);

        for current in 0..iter.size() {
            // VM reference: decompose current into iter indices, match dims by id
            // (DimB is id 1 in both), read source[that DimB index].
            let n = iter_rv.dims.len();
            let mut idx: SmallVec<[u16; 4]> = smallvec::smallvec![0; n];
            let mut rem = current;
            for d in (0..n).rev() {
                idx[d] = (rem % iter_rv.dims[d] as usize) as u16;
                rem /= iter_rv.dims[d] as usize;
            }
            // DimB is iteration axis 1.
            let want = src_rv.flat_offset(&[idx[1]]);
            assert_eq!(
                src.iter_broadcast_offset(&iter, current, &ctx2),
                Some(want),
                "element {current}"
            );
        }
    }
}
