// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::Ident;
use crate::datamodel;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NamedDimension {
    pub elements: Vec<String>,
    pub indexed_elements: HashMap<Ident, usize>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Dimension {
    Indexed(Ident, u32),
    Named(Ident, NamedDimension),
}

impl Dimension {
    pub fn len(&self) -> usize {
        match self {
            Dimension::Indexed(_, size) => *size as usize,
            Dimension::Named(_, named) => named.elements.len(),
        }
    }

    #[allow(unused)]
    pub fn name(&self) -> &str {
        match self {
            Dimension::Indexed(name, _) | Dimension::Named(name, _) => name,
        }
    }
}

impl From<datamodel::Dimension> for Dimension {
    fn from(dim: datamodel::Dimension) -> Dimension {
        match dim {
            datamodel::Dimension::Indexed(name, size) => Dimension::Indexed(name, size),
            datamodel::Dimension::Named(name, elements) => Dimension::Named(
                name,
                NamedDimension {
                    indexed_elements: elements
                        .iter()
                        .enumerate()
                        // system dynamic indexes are 1-indexed
                        .map(|(i, elem)| (elem.clone(), i + 1))
                        .collect(),
                    elements,
                },
            ),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct DimensionsContext {
    dimensions: HashMap<Ident, Dimension>,
}

impl DimensionsContext {
    pub(crate) fn from(dimensions: &[datamodel::Dimension]) -> DimensionsContext {
        DimensionsContext {
            dimensions: dimensions
                .iter()
                .map(|dim| (dim.name().to_owned(), Dimension::from(dim.clone())))
                .collect(),
        }
    }

    pub(crate) fn lookup(&self, element: &str) -> Option<u32> {
        if let Some(pos) = element.find('·') {
            let dimension_name = &element[..pos];
            let element_name = &element[pos + '·'.len_utf8()..];
            if let Some(Dimension::Named(_, dimension)) = self.dimensions.get(dimension_name) {
                if let Some(off) = dimension.indexed_elements.get(element_name) {
                    return Some(*off as u32);
                }
            }
        }
        None
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionRange {
    dim: Option<Dimension>,
    start: u32,
    end: u32,
}

#[allow(dead_code)]
impl DimensionRange {
    pub fn new(dim: Dimension, start: u32, end: u32) -> Self {
        DimensionRange {
            dim: Some(dim),
            start,
            end,
        }
    }

    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
}

/// DimensionInfo represents the array dimensions of an expression.
/// It uses the existing Dimension enum which already encapsulates
/// both name and size together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionVec {
    dims: Vec<DimensionRange>,
}

#[allow(dead_code)]
impl DimensionVec {
    /// Create dimension info from a vector of dimensions
    pub fn new(dims: Vec<DimensionRange>) -> Self {
        DimensionVec { dims }
    }
}

/// Dimension information for non-contiguous array views
///
/// `StridedDimension` is used only in `ArrayView::Strided` for views where
/// elements are not stored consecutively in memory (e.g., after transpose,
/// column/row selection, or slicing operations). For normal contiguous arrays,
/// `ArrayView::Contiguous` is used instead, which only needs dimension sizes
/// since strides can be computed implicitly assuming row-major order.
///
/// A `StridedDimension` describes how to iterate through one dimension of a
/// strided array view. The key insight is that `dimension.len()` represents
/// the number of elements in this dimension of the *view*, not the underlying
/// storage.
///
/// For example, if you have a 3x4 matrix and select column 1, you get a view
/// with shape [3]. The `StridedDimension` would be:
/// - `dimension`: a Dimension with length 3 (the view has 3 elements)
/// - `stride`: 4 (skip 4 elements in storage to get to the next row)
///
/// The stride tells you how many elements to skip in the underlying flat
/// storage to move by 1 in this dimension. For a contiguous row-major array,
/// the rightmost dimension has stride 1, and each dimension to the left has
/// a stride equal to the product of all dimension sizes to its right.
///
/// Example: A 2x3x4 array in row-major order has strides [12, 4, 1]:
/// - To move by 1 in the first dimension: skip 12 elements (3*4)
/// - To move by 1 in the second dimension: skip 4 elements (4)
/// - To move by 1 in the third dimension: skip 1 element
#[derive(PartialEq, Clone, Debug)]
pub struct StridedDimension {
    pub dimension: Dimension,
    pub stride: isize,
}
