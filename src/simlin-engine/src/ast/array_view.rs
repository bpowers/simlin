// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::common::Result;
use crate::sim_err;

/// Information about a sparse (non-contiguous) dimension in an array view.
/// Used when a subdimension's elements are not contiguous in the parent dimension.
#[derive(PartialEq, Clone, Debug)]
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
#[derive(PartialEq, Clone, Debug)]
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

    /// Check if this view represents contiguous data in row-major order
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

    /// Apply a range subscript to create a new view
    #[allow(dead_code)]
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

    /// Create a transposed view by reversing dimensions and strides.
    /// Transpose reverses the order of dimensions - e.g., a 2x3 array becomes 3x2.
    pub fn transpose(&self) -> ArrayView {
        let mut dims = self.dims.clone();
        dims.reverse();
        let mut strides = self.strides.clone();
        strides.reverse();
        let mut dim_names = self.dim_names.clone();
        dim_names.reverse();

        ArrayView {
            dims,
            strides,
            offset: self.offset,
            sparse: self.sparse.clone(),
            dim_names,
        }
    }

    /// Create a view with reordered dimensions.
    /// The reordering slice maps output dimension indices to input dimension indices.
    /// For example, [1, 0] swaps the first two dimensions (equivalent to transpose for 2D).
    /// [1, 2, 0] means output dim 0 = input dim 1, output dim 1 = input dim 2, output dim 2 = input dim 0.
    pub fn reorder_dimensions(&self, reordering: &[usize]) -> ArrayView {
        let dims: Vec<usize> = reordering.iter().map(|&idx| self.dims[idx]).collect();
        let strides: Vec<isize> = reordering.iter().map(|&idx| self.strides[idx]).collect();
        let dim_names: Vec<String> = reordering
            .iter()
            .map(|&idx| self.dim_names[idx].clone())
            .collect();

        ArrayView {
            dims,
            strides,
            offset: self.offset,
            sparse: self.sparse.clone(),
            dim_names,
        }
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

    #[test]
    fn test_non_contiguous_with_sparse() {
        let mut view = ArrayView::contiguous(vec![4, 4]);
        view.sparse.push(SparseInfo {
            dim_index: 0,
            parent_offsets: vec![0, 2],
        });
        assert!(!view.is_contiguous());
    }
}
