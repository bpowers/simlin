// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Only `apply_range_subscript` (test-only, below) returns a fallible result.
#[cfg(test)]
use crate::common::Result;
#[cfg(test)]
use crate::sim_err;

/// Information about a sparse (non-contiguous) dimension in an array view.
/// Used when a subdimension's elements are not contiguous in the parent dimension.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub struct SparseInfo {
    /// Which dimension (0-indexed) in the view is sparse
    pub dim_index: usize,
    /// Parent offsets to iterate (e.g., [0, 2] for elements at indices 0 and 2)
    pub parent_offsets: Vec<usize>,
}

/// Represents a view into array data with support for striding and slicing.
///
/// ArrayView enables efficient array operations without copying data by adjusting
/// how we iterate over existing data (changing offsets and strides) rather than
/// creating new arrays.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub struct ArrayView {
    /// Dimension sizes after slicing/viewing
    pub dims: Vec<usize>,
    /// Stride for each dimension (elements to skip to move by 1 in that dimension)
    pub strides: Vec<isize>,
    /// Starting offset in the underlying data
    pub offset: usize,
    /// Sparse dimension info (empty means fully contiguous)
    pub sparse: Vec<SparseInfo>,
    /// Dimension names for each dimension (canonical form).
    /// Used for dimension ID lookup in bytecode generation and broadcasting.
    /// Empty string means dimension name is unknown (e.g., temp arrays).
    pub dim_names: Vec<String>,
}

impl ArrayView {
    /// Create a contiguous array view (row-major order) with no dimension names
    pub fn contiguous(dims: Vec<usize>) -> Self {
        Self::contiguous_with_names(dims, Vec::new())
    }

    /// Create a contiguous array view (row-major order) with dimension names.
    ///
    /// # Panics
    /// Panics in debug builds if dim_names is non-empty and its length doesn't match dims.
    pub fn contiguous_with_names(dims: Vec<usize>, dim_names: Vec<String>) -> Self {
        let mut strides = vec![1isize; dims.len()];
        // Build strides from right to left for row-major order
        for i in (0..dims.len().saturating_sub(1)).rev() {
            strides[i] = strides[i + 1] * dims[i + 1] as isize;
        }

        // Validate dimension names match dimensions
        // An empty dim_names vector is allowed (means no names provided)
        // A non-empty vector must have exactly the right length
        debug_assert!(
            dim_names.is_empty() || dim_names.len() == dims.len(),
            "dim_names length ({}) must match dims length ({}) when provided",
            dim_names.len(),
            dims.len()
        );

        // If dim_names is empty, fill with empty strings to maintain invariant
        let dim_names = if dim_names.is_empty() {
            vec![String::new(); dims.len()]
        } else {
            dim_names
        };

        ArrayView {
            dims,
            strides,
            offset: 0,
            sparse: Vec::new(),
            dim_names,
        }
    }

    /// Total number of elements in the view
    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.dims.iter().product()
    }

    /// Check if this view represents contiguous data in row-major order.
    ///
    /// Test-only: production asks the same question of `RuntimeView` /
    /// `wasmgen::ViewDesc` / `dimensions::SubdimensionRelation`, never of an
    /// `ast::ArrayView`.
    #[cfg(test)]
    pub fn is_contiguous(&self) -> bool {
        if self.offset != 0 || !self.sparse.is_empty() {
            return false;
        }

        let mut expected_stride = 1isize;
        for i in (0..self.dims.len()).rev() {
            if self.strides[i] != expected_stride {
                return false;
            }
            expected_stride *= self.dims[i] as isize;
        }
        true
    }

    /// Apply a range subscript to create a new view.
    ///
    /// Test-only: range subscripts are lowered by `compiler::subscript`, which
    /// does not route through this helper.
    #[cfg(test)]
    pub fn apply_range_subscript(
        &self,
        dim_index: usize,
        start: usize,
        end: usize,
    ) -> Result<ArrayView> {
        if dim_index >= self.dims.len() {
            return sim_err!(Generic, "dimension index out of bounds".to_string());
        }
        if start >= end || end > self.dims[dim_index] {
            return sim_err!(Generic, "invalid range bounds".to_string());
        }

        let mut new_dims = self.dims.clone();
        new_dims[dim_index] = end - start;

        let new_strides = self.strides.clone();
        let new_offset = self.offset + (start * self.strides[dim_index] as usize);

        Ok(ArrayView {
            dims: new_dims,
            strides: new_strides,
            offset: new_offset,
            sparse: self.sparse.clone(),
            dim_names: self.dim_names.clone(),
        })
    }

    /// This view with its axes in the order `order` gives: axis `i` of the
    /// result is axis `order[i]` of `self`.
    ///
    /// The one place a dimension-reordering transform is written, because
    /// FOUR things move together and a transform that moves three of them
    /// silently addresses the wrong axis: `dims`, `strides`, `dim_names`, and
    /// every [`SparseInfo::dim_index`], which is an index INTO `dims` and so
    /// has to be renumbered through the same permutation. Transposing
    /// `arr[*:Sub, *]` without renumbering left the mapping claiming the other
    /// axis was the sparse one, and `SUM` over the result read all-NaN with
    /// exit 0 (GH #1027). The runtime transforms that drop an axis
    /// (`bytecode::RuntimeView::apply_single_subscript`,
    /// `wasmgen::ViewDesc::apply_single_subscript_dynamic`) renumber for the
    /// same reason.
    ///
    /// # Panics
    /// Panics in debug builds if `order` is not a permutation of this view's
    /// axes, or if the result's sparse mappings do not describe its axes.
    fn permute_axes(&self, order: &[usize]) -> ArrayView {
        debug_assert_eq!(
            order.len(),
            self.dims.len(),
            "order length ({}) must match number of dimensions ({})",
            order.len(),
            self.dims.len()
        );
        debug_assert!(
            order.iter().all(|&idx| idx < self.dims.len()),
            "order entries must be valid dimension indices (< {})",
            self.dims.len()
        );

        // `order` maps NEW axis -> OLD axis; a sparse mapping names an OLD
        // axis and needs the inverse.
        let mut old_to_new = vec![None; self.dims.len()];
        for (new_idx, &old_idx) in order.iter().enumerate() {
            old_to_new[old_idx] = Some(new_idx);
        }

        let view = ArrayView {
            dims: order.iter().map(|&idx| self.dims[idx]).collect(),
            strides: order.iter().map(|&idx| self.strides[idx]).collect(),
            offset: self.offset,
            sparse: self
                .sparse
                .iter()
                .filter_map(|s| {
                    Some(SparseInfo {
                        dim_index: old_to_new.get(s.dim_index).copied().flatten()?,
                        parent_offsets: s.parent_offsets.clone(),
                    })
                })
                .collect(),
            dim_names: order
                .iter()
                .map(|&idx| self.dim_names[idx].clone())
                .collect(),
        };
        view.debug_assert_sparse_describes_axes();
        view
    }

    /// Every sparse mapping names an axis of this view and supplies exactly
    /// one parent offset per element of it.
    ///
    /// `build_view_from_ops` establishes both when it emits a `SparseRange`;
    /// this is what a transform has to preserve.
    fn debug_assert_sparse_describes_axes(&self) {
        debug_assert!(
            self.sparse.iter().all(|s| s.dim_index < self.dims.len()
                && s.parent_offsets.len() == self.dims[s.dim_index]),
            "sparse mapping must describe one of this view's axes: dims {:?}, sparse {:?}",
            self.dims,
            self.sparse
                .iter()
                .map(|s| (s.dim_index, s.parent_offsets.len()))
                .collect::<Vec<_>>()
        );
    }

    /// A transposed view: the axes in reverse order, so a 2x3 becomes a 3x2.
    pub fn transpose(&self) -> ArrayView {
        let order: Vec<usize> = (0..self.dims.len()).rev().collect();
        self.permute_axes(&order)
    }

    /// A view with reordered dimensions: output axis `i` is input axis
    /// `reordering[i]`, so `[1, 0]` swaps the first two (a 2-D transpose) and
    /// `[1, 2, 0]` moves the first axis to the end.
    ///
    /// # Panics
    /// Panics in debug builds if `reordering` is not a permutation of this
    /// view's axes.
    pub fn reorder_dimensions(&self, reordering: &[usize]) -> ArrayView {
        self.permute_axes(reordering)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contiguous_1d() {
        let view = ArrayView::contiguous(vec![5]);
        assert_eq!(view.dims, vec![5]);
        assert_eq!(view.strides, vec![1]);
        assert_eq!(view.offset, 0);
        assert!(view.sparse.is_empty());
        assert_eq!(view.size(), 5);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_contiguous_2d() {
        let view = ArrayView::contiguous(vec![3, 4]);
        assert_eq!(view.dims, vec![3, 4]);
        assert_eq!(view.strides, vec![4, 1]);
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 12);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_contiguous_3d() {
        let view = ArrayView::contiguous(vec![2, 3, 4]);
        assert_eq!(view.dims, vec![2, 3, 4]);
        assert_eq!(view.strides, vec![12, 4, 1]);
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 24);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_apply_range_subscript() {
        let view = ArrayView::contiguous(vec![5, 4]);
        let sliced = view.apply_range_subscript(0, 1, 3).unwrap();
        assert_eq!(sliced.dims, vec![2, 4]);
        assert_eq!(sliced.strides, vec![4, 1]);
        assert_eq!(sliced.offset, 4); // 1 * stride[0] = 1 * 4 = 4
        assert!(!sliced.is_contiguous()); // offset != 0
    }

    #[test]
    fn test_apply_range_subscript_second_dim() {
        let view = ArrayView::contiguous(vec![3, 6]);
        let sliced = view.apply_range_subscript(1, 2, 5).unwrap();
        assert_eq!(sliced.dims, vec![3, 3]);
        assert_eq!(sliced.strides, vec![6, 1]);
        assert_eq!(sliced.offset, 2);
    }

    #[test]
    fn test_apply_range_subscript_invalid() {
        let view = ArrayView::contiguous(vec![5, 4]);

        // Out of bounds dimension
        assert!(view.apply_range_subscript(2, 0, 1).is_err());

        // Invalid range (start >= end)
        assert!(view.apply_range_subscript(0, 3, 2).is_err());

        // End exceeds dimension size
        assert!(view.apply_range_subscript(0, 0, 6).is_err());
    }

    #[test]
    fn test_non_contiguous_with_offset() {
        let mut view = ArrayView::contiguous(vec![4, 4]);
        view.offset = 2;
        assert!(!view.is_contiguous());
    }

    /// A star range over a non-contiguous subdimension: axis 0 has two
    /// elements drawn from parent offsets 0 and 2, the shape
    /// `compiler::subscript::build_view_from_ops` emits for `arr[*:Sub, *]`.
    fn sparse_star_range() -> ArrayView {
        let mut view = ArrayView::contiguous_with_names(
            vec![2, 3],
            vec!["parent".to_string(), "other".to_string()],
        );
        // The view selects rows 0 and 2 of a 4x3 parent, so it keeps the
        // parent's strides rather than its own contiguous ones.
        view.strides = vec![3, 1];
        view.sparse.push(SparseInfo {
            dim_index: 0,
            parent_offsets: vec![0, 2],
        });
        view
    }

    #[test]
    fn test_non_contiguous_with_sparse() {
        assert!(!sparse_star_range().is_contiguous());
    }

    /// GH #1027: a sparse mapping's `dim_index` indexes the view's `dims`, so
    /// a transform that moves the axes has to renumber it. Transposing
    /// `arr[*:Sub, *]` used to leave the mapping on axis 0 -- claiming the
    /// size-3 dense axis was the sparse one -- and every element of the result
    /// then addressed the wrong storage.
    #[test]
    fn transpose_renumbers_a_sparse_axis() {
        let view = sparse_star_range();
        let transposed = view.transpose();

        assert_eq!(transposed.dims, vec![3, 2]);
        assert_eq!(transposed.strides, vec![1, 3]);
        assert_eq!(transposed.dim_names, vec!["other", "parent"]);
        assert_eq!(transposed.sparse.len(), 1);
        assert_eq!(transposed.sparse[0].dim_index, 1);
        assert_eq!(transposed.sparse[0].parent_offsets, vec![0, 2]);
        assert_eq!(
            transposed.sparse[0].parent_offsets.len(),
            transposed.dims[transposed.sparse[0].dim_index],
            "a mapping must supply one parent offset per element of its axis"
        );
    }

    /// The same for the general reordering, on a 3-D view where the sparse
    /// axis moves twice.
    #[test]
    fn reorder_dimensions_renumbers_a_sparse_axis() {
        let mut view = ArrayView::contiguous_with_names(
            vec![2, 3, 4],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        view.sparse.push(SparseInfo {
            dim_index: 0,
            parent_offsets: vec![0, 2],
        });

        // [1, 2, 0] moves the sparse axis from position 0 to position 2.
        let rotated = view.reorder_dimensions(&[1, 2, 0]);
        assert_eq!(rotated.dims, vec![3, 4, 2]);
        assert_eq!(rotated.sparse[0].dim_index, 2);
        assert_eq!(rotated.sparse[0].parent_offsets, vec![0, 2]);

        // The identity leaves it where it is.
        let identity = view.reorder_dimensions(&[0, 1, 2]);
        assert_eq!(identity.sparse[0].dim_index, 0);
    }

    /// Two sparse axes are renumbered independently, so the mapping that
    /// describes each axis follows that axis rather than its position.
    #[test]
    fn transpose_renumbers_every_sparse_axis() {
        let mut view = ArrayView::contiguous_with_names(
            vec![2, 4, 3],
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
        view.sparse.push(SparseInfo {
            dim_index: 0,
            parent_offsets: vec![0, 2],
        });
        view.sparse.push(SparseInfo {
            dim_index: 2,
            parent_offsets: vec![1, 4, 5],
        });

        let transposed = view.transpose();
        assert_eq!(transposed.dims, vec![3, 4, 2]);
        assert_eq!(transposed.sparse[0].dim_index, 2);
        assert_eq!(transposed.sparse[0].parent_offsets, vec![0, 2]);
        assert_eq!(transposed.sparse[1].dim_index, 0);
        assert_eq!(transposed.sparse[1].parent_offsets, vec![1, 4, 5]);
    }

    #[test]
    fn test_transpose_2d() {
        let view =
            ArrayView::contiguous_with_names(vec![2, 3], vec!["A".to_string(), "B".to_string()]);
        let transposed = view.transpose();

        assert_eq!(transposed.dims, vec![3, 2]);
        assert_eq!(transposed.strides, vec![1, 3]); // Reversed from [3, 1]
        assert_eq!(transposed.dim_names, vec!["B", "A"]);
        assert_eq!(transposed.offset, 0);
    }

    #[test]
    fn test_transpose_3d() {
        let view = ArrayView::contiguous_with_names(
            vec![2, 3, 4],
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );
        let transposed = view.transpose();

        assert_eq!(transposed.dims, vec![4, 3, 2]);
        assert_eq!(transposed.strides, vec![1, 4, 12]); // Reversed from [12, 4, 1]
        assert_eq!(transposed.dim_names, vec!["C", "B", "A"]);
    }

    #[test]
    fn test_transpose_preserves_offset_and_sparse() {
        let mut view = sparse_star_range();
        view.offset = 5;

        let transposed = view.transpose();

        assert_eq!(transposed.offset, 5);
        assert_eq!(transposed.sparse.len(), 1);
    }

    #[test]
    fn test_reorder_dimensions_swap() {
        let view =
            ArrayView::contiguous_with_names(vec![2, 3], vec!["A".to_string(), "B".to_string()]);
        // Swap dimensions: [1, 0] is equivalent to transpose for 2D
        let reordered = view.reorder_dimensions(&[1, 0]);

        assert_eq!(reordered.dims, vec![3, 2]);
        assert_eq!(reordered.strides, vec![1, 3]);
        assert_eq!(reordered.dim_names, vec!["B", "A"]);
    }

    #[test]
    fn test_reorder_dimensions_3d() {
        let view = ArrayView::contiguous_with_names(
            vec![2, 3, 4],
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );
        // Rotate dimensions: [1, 2, 0] moves first dim to end
        let reordered = view.reorder_dimensions(&[1, 2, 0]);

        assert_eq!(reordered.dims, vec![3, 4, 2]);
        assert_eq!(reordered.strides, vec![4, 1, 12]);
        assert_eq!(reordered.dim_names, vec!["B", "C", "A"]);
    }

    #[test]
    fn test_reorder_dimensions_identity() {
        let view = ArrayView::contiguous_with_names(
            vec![2, 3, 4],
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );
        // Identity reordering: [0, 1, 2]
        let reordered = view.reorder_dimensions(&[0, 1, 2]);

        assert_eq!(reordered.dims, view.dims);
        assert_eq!(reordered.strides, view.strides);
        assert_eq!(reordered.dim_names, view.dim_names);
    }

    #[test]
    fn test_reorder_dimensions_preserves_offset() {
        let mut view = ArrayView::contiguous(vec![2, 3]);
        view.offset = 10;

        let reordered = view.reorder_dimensions(&[1, 0]);

        assert_eq!(reordered.offset, 10);
    }
}
