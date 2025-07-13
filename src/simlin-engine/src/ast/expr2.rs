// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{walk_builtin_expr, BuiltinContents, BuiltinFn, Loc};
use crate::common::{EquationResult, Ident};
use crate::dimensions::{Dimension, NamedDimension, StridedDimension};
use crate::eqn_err;
use float_cmp::approx_eq;
use std::collections::HashMap;
use std::iter::Iterator;

/// Represents different ways to view array data in memory
///
/// An `ArrayView` describes the shape and access pattern for array data without
/// owning the data itself. This allows efficient operations like slicing and
/// transposing without copying data.
#[derive(PartialEq, Clone, Debug)]
pub enum ArrayView {
    /// Simple contiguous array in row-major order
    ///
    /// All elements are stored consecutively in memory. For a 2D array [row][col],
    /// elements are laid out as: [0,0], [0,1], [0,2], ..., [1,0], [1,1], ...
    Contiguous { dims: Vec<Dimension> },

    /// Strided array view (for transposes, slices, etc.)
    ///
    /// Elements may not be consecutive in memory. Each dimension has an associated
    /// stride that determines how many elements to skip to move by 1 in that dimension.
    /// The `offset` indicates where in the underlying storage this view starts.
    ///
    /// For example, selecting column 1 of a 3x4 matrix creates a strided view:
    /// - dims: [StridedDimension { dimension: Dimension(3), stride: 4 }]
    /// - offset: 1 (start at element [0,1])
    ///
    /// This gives us elements at positions 1, 5, 9 in the underlying storage.
    Strided {
        dims: Vec<StridedDimension>,
        offset: usize,
    },
}

impl ArrayView {
    /// Returns the total number of elements in the array
    pub fn size(&self) -> usize {
        match self {
            ArrayView::Contiguous { dims: dimensions } => {
                dimensions.iter().map(|d| d.len()).product()
            }
            ArrayView::Strided {
                dims: dim_strides, ..
            } => dim_strides.iter().map(|ds| ds.dimension.len()).product(),
        }
    }

    /// Check if this view represents contiguous data in row-major order
    pub fn is_contiguous(&self) -> bool {
        match self {
            ArrayView::Contiguous { .. } => true,
            ArrayView::Strided { dims, offset } => {
                // Must start at beginning of data
                if *offset != 0 {
                    return false;
                }

                // Check if strides match row-major order
                let mut expected_stride = 1isize;
                for sd in dims.iter().rev() {
                    if sd.stride != expected_stride {
                        return false;
                    }
                    expected_stride *= sd.dimension.len() as isize;
                }
                true
            }
        }
    }

    /// Get the shape as a vector of dimension sizes
    pub fn shape(&self) -> Vec<usize> {
        match self {
            ArrayView::Contiguous { dims } => dims.iter().map(|d| d.len()).collect(),
            ArrayView::Strided { dims, .. } => dims.iter().map(|sd| sd.dimension.len()).collect(),
        }
    }

    /// Get the dimensions (without stride information)
    pub fn dimensions(&self) -> Vec<&Dimension> {
        match self {
            ArrayView::Contiguous { dims } => dims.iter().collect(),
            ArrayView::Strided { dims, .. } => dims.iter().map(|sd| &sd.dimension).collect(),
        }
    }

    /// Apply subscript operation to create a new view
    pub fn subscript(&self, indices: &[IndexExpr2]) -> EquationResult<ArrayView> {
        match self {
            ArrayView::Contiguous { dims } => {
                // Calculate strides for row-major memory layout.
                // In row-major layout, the last dimension varies fastest in memory.
                // For a 3D array with shape [2, 3, 4], elements are stored as:
                // [0,0,0] [0,0,1] [0,0,2] [0,0,3] [0,1,0] [0,1,1] ... [1,2,3]
                //
                // A stride is how many elements to skip to move by 1 in that dimension:
                // - To go from [0,0,0] to [0,0,1]: skip 1 element (stride = 1)
                // - To go from [0,0,0] to [0,1,0]: skip 4 elements (stride = 4)
                // - To go from [0,0,0] to [1,0,0]: skip 12 elements (stride = 12)
                //
                // We build strides right-to-left since each dimension's stride depends
                // on the sizes of all dimensions to its right:
                // - Last dimension: stride = 1 (always)
                // - Each previous: stride = next_stride * next_dimension_size
                let mut original_strides = vec![1isize; dims.len()];
                for i in (0..dims.len() - 1).rev() {
                    original_strides[i] = original_strides[i + 1] * dims[i + 1].len() as isize;
                }

                // Check that we have exactly the right number of indices
                if indices.len() != dims.len() {
                    // Use the location from the first index if available, otherwise default
                    let loc = indices.first().map(|idx| idx.get_loc()).unwrap_or_default();
                    return eqn_err!(Generic, loc.start, loc.end);
                }

                let mut new_dims = Vec::new();
                let mut offset = 0usize;

                for (i, index_expr) in indices.iter().enumerate() {
                    match index_expr {
                        IndexExpr2::Wildcard(_) => {
                            // Keep this dimension
                            new_dims.push(StridedDimension {
                                dimension: dims[i].clone(),
                                stride: original_strides[i],
                            });
                        }
                        IndexExpr2::Expr(expr) => {
                            let loc = expr.get_loc();

                            if let Expr2::Var(id, _, _loc) = expr {
                                // Case 1: Check if id matches the current dimension name
                                if id == dims[i].name() {
                                    // Keep this dimension (select all elements)
                                    new_dims.push(StridedDimension {
                                        dimension: dims[i].clone(),
                                        stride: original_strides[i],
                                    });
                                } else {
                                    // Case 2: Check if id is a subscript element of the current dimension
                                    let mut found_element = false;
                                    let mut element_index = 0usize;

                                    if let Dimension::Named(_, named_dim) = &dims[i] {
                                        // Check if id matches any element name in this dimension
                                        if let Some(idx) = named_dim.indexed_elements.get(id) {
                                            found_element = true;
                                            // indexed_elements are 1-based, convert to 0-based
                                            element_index = idx - 1;
                                        }
                                    }

                                    if found_element {
                                        // Skip this dimension and adjust offset
                                        offset += element_index * original_strides[i] as usize;
                                    } else {
                                        // Case 3: id is a variable - we don't know the specific index at compile time
                                        // but we still skip this dimension (reducing dimensionality)
                                        // The actual offset will be computed at runtime

                                        // For now, we skip this dimension without adding to offset
                                        // This represents a dynamic index that will be resolved at runtime

                                        // TODO: eventually we probably need something here that is more explicit about "dynamic"
                                    }
                                }
                            } else {
                                // Evaluate the expression to get an integer index
                                let idx = const_int_eval(expr)?;

                                if idx < 0 || idx as usize >= dims[i].len() {
                                    return eqn_err!(Generic, loc.start, loc.end);
                                }
                                // Skip this dimension and adjust offset
                                offset += idx as usize * original_strides[i] as usize;
                            }
                        }
                        IndexExpr2::Range(start, end, range_loc) => {
                            // Extract start and end values
                            let start_idx = const_int_eval(start)?;
                            let end_idx = const_int_eval(end)?;

                            if start_idx < 0 || end_idx < 0 {
                                return eqn_err!(Generic, range_loc.start, range_loc.end);
                            }

                            let start_idx = start_idx as usize;
                            let end_idx = end_idx as usize;

                            if start_idx >= dims[i].len() || end_idx > dims[i].len() {
                                return eqn_err!(Generic, range_loc.start, range_loc.end);
                            }

                            if start_idx >= end_idx {
                                return eqn_err!(Generic, range_loc.start, range_loc.end);
                            }

                            // Create a new dimension for the slice
                            let new_dim = match &dims[i] {
                                Dimension::Indexed(name, _) => Dimension::Indexed(
                                    format!("{name}[{start_idx}:{end_idx}]"),
                                    (end_idx - start_idx) as u32,
                                ),
                                Dimension::Named(name, named) => {
                                    let new_elements: Vec<String> =
                                        named.elements[start_idx..end_idx].to_vec();
                                    let mut indexed_elements = HashMap::new();
                                    for (i, elem) in new_elements.iter().enumerate() {
                                        indexed_elements.insert(elem.clone(), i + 1);
                                    }
                                    Dimension::Named(
                                        format!("{name}[{start_idx}:{end_idx}]"),
                                        NamedDimension {
                                            elements: new_elements,
                                            indexed_elements,
                                        },
                                    )
                                }
                            };

                            new_dims.push(StridedDimension {
                                dimension: new_dim,
                                stride: original_strides[i],
                            });

                            // Adjust offset for the start of the range
                            offset += start_idx * original_strides[i] as usize;
                        }
                        IndexExpr2::StarRange(_, loc) => {
                            return eqn_err!(TodoStarRange, loc.start, loc.end);
                        }
                        IndexExpr2::DimPosition(_, loc) => {
                            // Dimension position operators are not valid in array views
                            // They should only be used in assignment contexts
                            return eqn_err!(Generic, loc.start, loc.end);
                        }
                    }
                }

                // If we consumed all indices and reduced all dimensions, return a scalar view
                if new_dims.is_empty() {
                    // This represents a scalar - could return Contiguous with empty dims
                    Ok(ArrayView::Contiguous { dims: vec![] })
                } else {
                    Ok(ArrayView::Strided {
                        dims: new_dims,
                        offset,
                    })
                }
            }
            ArrayView::Strided { .. } => {
                // For now, don't support subscripting already strided arrays
                // Use a default location since we don't have a specific index location here
                eqn_err!(Generic, 0, 0)
            }
        }
    }

    /// Creates an iterator over the array elements in logical order
    pub fn iter<'a>(&self, data: &'a [f64]) -> ArrayIterator<'a> {
        match self {
            ArrayView::Contiguous { dims: dimensions } => {
                ArrayIterator::new_contiguous(data, dimensions)
            }
            ArrayView::Strided {
                dims: dim_strides,
                offset,
            } => ArrayIterator::new_strided(data, dim_strides, *offset),
        }
    }

    /// Creates a transposed view (reverses dimensions)
    pub fn transpose(&self) -> Self {
        match self {
            ArrayView::Contiguous { dims: dimensions } => {
                if dimensions.is_empty() {
                    // Scalar case - transpose is identity
                    return ArrayView::Contiguous {
                        dims: dimensions.clone(),
                    };
                }

                // Calculate strides for the original shape in row-major order
                let mut dim_strides = Vec::with_capacity(dimensions.len());
                let mut stride = 1isize;

                // Build strides from right to left
                for dim in dimensions.iter().rev() {
                    dim_strides.push(StridedDimension {
                        dimension: dim.clone(),
                        stride,
                    });
                    stride *= dim.len() as isize;
                }

                // Reverse to get original order, then reverse again for transpose
                dim_strides.reverse();
                dim_strides.reverse();

                ArrayView::Strided {
                    dims: dim_strides,
                    offset: 0,
                }
            }
            ArrayView::Strided {
                dims: dim_strides,
                offset,
            } => {
                let mut new_dim_strides = dim_strides.clone();
                new_dim_strides.reverse();

                ArrayView::Strided {
                    dims: new_dim_strides,
                    offset: *offset,
                }
            }
        }
    }
}

/// Iterator over array elements in logical order
pub struct ArrayIterator<'a> {
    data: &'a [f64],
    shape: Vec<usize>,
    strides: Vec<isize>,
    offset: usize,
    indices: Vec<usize>,
    done: bool,
}

impl<'a> ArrayIterator<'a> {
    fn new_contiguous(data: &'a [f64], dimensions: &[Dimension]) -> Self {
        // Calculate strides for contiguous row-major array
        let mut strides = Vec::with_capacity(dimensions.len());
        let mut shape = Vec::with_capacity(dimensions.len());
        let mut stride = 1isize;

        for dim in dimensions.iter().rev() {
            let size = dim.len();
            shape.push(size);
            strides.push(stride);
            stride *= size as isize;
        }
        shape.reverse();
        strides.reverse();

        let indices = vec![0; dimensions.len()];
        let done = shape.contains(&0);

        ArrayIterator {
            data,
            shape,
            strides,
            offset: 0,
            indices,
            done,
        }
    }

    fn new_strided(data: &'a [f64], dim_strides: &[StridedDimension], offset: usize) -> Self {
        let shape: Vec<usize> = dim_strides.iter().map(|ds| ds.dimension.len()).collect();
        let strides: Vec<isize> = dim_strides.iter().map(|ds| ds.stride).collect();
        let indices = vec![0; dim_strides.len()];
        let done = shape.contains(&0);

        ArrayIterator {
            data,
            shape,
            strides,
            offset,
            indices,
            done,
        }
    }

    fn current_offset(&self) -> usize {
        let mut offset = self.offset;
        for (i, &idx) in self.indices.iter().enumerate() {
            offset = (offset as isize + idx as isize * self.strides[i]) as usize;
        }
        offset
    }

    fn increment(&mut self) {
        if self.indices.is_empty() {
            // Scalar case - just mark as done after one iteration
            self.done = true;
            return;
        }

        // Increment indices from right to left (last dimension varies fastest)
        for i in (0..self.indices.len()).rev() {
            self.indices[i] += 1;
            if self.indices[i] < self.shape[i] {
                return;
            }
            self.indices[i] = 0;
        }
        // If we get here, we've wrapped around all dimensions
        self.done = true;
    }
}

impl<'a> Iterator for ArrayIterator<'a> {
    type Item = f64;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        let offset = self.current_offset();
        let value = self.data.get(offset).copied();

        self.increment();

        value
    }
}

#[derive(PartialEq, Clone, Debug)]
pub enum ArraySource {
    Named(Ident, ArrayView),
    Temp(u32, ArrayView),
}

/// IndexExpr1 represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr2 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(Ident, Loc),
    Range(Expr2, Expr2, Loc),
    DimPosition(u32, Loc),
    Expr(Expr2),
}

impl IndexExpr2 {
    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            IndexExpr2::Wildcard(loc) => *loc,
            IndexExpr2::StarRange(_, loc) => *loc,
            IndexExpr2::Range(_, _, loc) => *loc,
            IndexExpr2::DimPosition(_, loc) => *loc,
            IndexExpr2::Expr(e) => e.get_loc(),
        }
    }

    pub(crate) fn from<C: Expr2Context>(expr: IndexExpr1, ctx: &mut C) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr1::Wildcard(loc) => IndexExpr2::Wildcard(loc),
            IndexExpr1::StarRange(ident, loc) => IndexExpr2::StarRange(ident, loc),
            IndexExpr1::Range(l, r, loc) => {
                IndexExpr2::Range(Expr2::from(l, ctx)?, Expr2::from(r, ctx)?, loc)
            }
            IndexExpr1::DimPosition(n, loc) => IndexExpr2::DimPosition(n, loc),
            IndexExpr1::Expr(e) => IndexExpr2::Expr(Expr2::from(e, ctx)?),
        };

        Ok(expr)
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            IndexExpr2::Wildcard(_) => None,
            IndexExpr2::StarRange(v, loc) => {
                if v == ident {
                    Some(*loc)
                } else {
                    None
                }
            }
            IndexExpr2::Range(l, r, _) => {
                if let Some(loc) = l.get_var_loc(ident) {
                    return Some(loc);
                }
                r.get_var_loc(ident)
            }
            IndexExpr2::DimPosition(_, _) => None,
            IndexExpr2::Expr(e) => e.get_var_loc(ident),
        }
    }
}

/// Expr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[allow(dead_code)]
#[derive(PartialEq, Clone, Debug)]
pub enum Expr2 {
    Const(String, f64, Loc),
    Var(Ident, Option<ArraySource>, Loc),
    App(BuiltinFn<Expr2>, Option<ArraySource>, Loc),
    Subscript(Ident, Vec<IndexExpr2>, Option<ArraySource>, Loc),
    Op1(UnaryOp, Box<Expr2>, Option<ArraySource>, Loc),
    Op2(BinaryOp, Box<Expr2>, Box<Expr2>, Option<ArraySource>, Loc),
    If(Box<Expr2>, Box<Expr2>, Box<Expr2>, Option<ArraySource>, Loc),
}

/// Context trait for converting Expr1 to Expr2
/// Provides access to variable dimension information and temp ID allocation
pub trait Expr2Context {
    /// Get the dimensions of a variable, or None if it's a scalar
    fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>>;

    /// Allocate a new temp ID for the current equation
    fn allocate_temp_id(&mut self) -> u32;
}

impl Expr2 {
    /// Extract the array source from an expression, if it has one
    fn get_array_source(&self) -> Option<&ArraySource> {
        match self {
            Expr2::Const(_, _, _) => None,
            Expr2::Var(_, array_source, _) => array_source.as_ref(),
            Expr2::App(_, array_source, _) => array_source.as_ref(),
            Expr2::Subscript(_, _, array_source, _) => array_source.as_ref(),
            Expr2::Op1(_, _, array_source, _) => array_source.as_ref(),
            Expr2::Op2(_, _, _, array_source, _) => array_source.as_ref(),
            Expr2::If(_, _, _, array_source, _) => array_source.as_ref(),
        }
    }

    /// Allocates a new temp ID and ensures the ArrayView is contiguous
    /// to avoid wasting allocated space
    fn allocate_temp_array<C: Expr2Context>(ctx: &mut C, view: &ArrayView) -> ArraySource {
        let contiguous_view = match view {
            ArrayView::Contiguous { .. } => view.clone(),
            ArrayView::Strided { dims, .. } => {
                // Convert strided view to contiguous by extracting dimensions
                // The dimension sizes already represent the view's shape
                let dimensions: Vec<Dimension> =
                    dims.iter().map(|sd| sd.dimension.clone()).collect();
                ArrayView::Contiguous { dims: dimensions }
            }
        };
        ArraySource::Temp(ctx.allocate_temp_id(), contiguous_view)
    }

    fn unify_array_sources<C: Expr2Context>(
        ctx: &mut C,
        l: Option<&ArraySource>,
        r: Option<&ArraySource>,
        loc: Loc,
    ) -> EquationResult<Option<ArraySource>> {
        match (l, r) {
            // Both sides are arrays - check dimensions match
            (Some(ArraySource::Named(_, view1)), Some(ArraySource::Named(_, view2)))
            | (Some(ArraySource::Named(_, view1)), Some(ArraySource::Temp(_, view2)))
            | (Some(ArraySource::Temp(_, view1)), Some(ArraySource::Named(_, view2)))
            | (Some(ArraySource::Temp(_, view1)), Some(ArraySource::Temp(_, view2))) => {
                let view = Self::unify_views(view1, view2, loc)?;
                Ok(Some(Self::allocate_temp_array(ctx, &view)))
            }
            // Left is array, right is scalar - broadcast
            (Some(ArraySource::Named(_, view)), None)
            | (Some(ArraySource::Temp(_, view)), None) => {
                Ok(Some(Self::allocate_temp_array(ctx, view)))
            }
            // Right is array, left is scalar - broadcast
            (None, Some(ArraySource::Named(_, view)))
            | (None, Some(ArraySource::Temp(_, view))) => {
                Ok(Some(Self::allocate_temp_array(ctx, view)))
            }
            // Both scalars
            (None, None) => Ok(None),
        }
    }

    /// Check if two array views have compatible dimensions for element-wise operations
    fn unify_views(a: &ArrayView, b: &ArrayView, loc: Loc) -> EquationResult<ArrayView> {
        match (a, b) {
            (ArrayView::Contiguous { dims: dims1 }, ArrayView::Contiguous { dims: dims2 }) => {
                if dims1.len() != dims2.len() {
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }
                let dims: EquationResult<Vec<Dimension>> = dims1
                    .iter()
                    .zip(dims2.iter())
                    .map(|(d1, d2)| {
                        if d1 == d2 {
                            Ok(d1.clone())
                        } else if d1.len() == d2.len() {
                            Ok(Dimension::Indexed("erased".into(), d1.len() as u32))
                        } else {
                            eqn_err!(MismatchedDimensions, loc.start, loc.end)
                        }
                    })
                    .collect();
                let dims = dims?;
                Ok(ArrayView::Contiguous { dims })
            }
            (
                ArrayView::Strided {
                    dims: dims1,
                    offset: off1,
                },
                ArrayView::Strided {
                    dims: dims2,
                    offset: off2,
                },
            ) => {
                if dims1.len() != dims2.len() || off1 != off2 {
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }

                let dims: EquationResult<Vec<StridedDimension>> = dims1
                    .iter()
                    .zip(dims2.iter())
                    .map(|(d1, d2)| {
                        if d1 == d2 {
                            Ok(d1.clone())
                        } else if d1.dimension.len() == d2.dimension.len() {
                            Ok(StridedDimension {
                                dimension: Dimension::Indexed(
                                    "erased".into(),
                                    d1.dimension.len() as u32,
                                ),
                                stride: d1.stride,
                            })
                        } else {
                            eqn_err!(MismatchedDimensions, loc.start, loc.end)
                        }
                    })
                    .collect();
                let dims = dims?;
                Ok(ArrayView::Strided {
                    dims,
                    offset: *off1,
                })
            }
            (
                ArrayView::Contiguous { dims },
                ArrayView::Strided {
                    dims: strided_dims,
                    offset: strided_off,
                },
            )
            | (
                ArrayView::Strided {
                    dims: strided_dims,
                    offset: strided_off,
                },
                ArrayView::Contiguous { dims },
            ) => {
                // TODO: I don't think strided off needs to strictly be zero
                if dims.len() != strided_dims.len() || *strided_off != 0 {
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }

                let unified_dims: EquationResult<Vec<StridedDimension>> = dims
                    .iter()
                    .zip(strided_dims.iter())
                    .map(|(d, sd)| {
                        if d.name() == sd.dimension.name() && d.len() == sd.dimension.len() {
                            Ok(sd.clone())
                        } else if d.len() == sd.dimension.len() {
                            Ok(StridedDimension {
                                dimension: Dimension::Indexed("erased".into(), d.len() as u32),
                                stride: sd.stride,
                            })
                        } else {
                            eqn_err!(MismatchedDimensions, loc.start, loc.end)
                        }
                    })
                    .collect();
                let unified_dims = unified_dims?;
                Ok(ArrayView::Strided {
                    dims: unified_dims,
                    offset: *strided_off,
                })
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn from<C: Expr2Context>(expr: Expr1, ctx: &mut C) -> EquationResult<Self> {
        let expr = match expr {
            Expr1::Const(s, n, loc) => Expr2::Const(s, n, loc),
            Expr1::Var(id, loc) => {
                let array_source = if let Some(dims) = ctx.get_dimensions(&id) {
                    let array_view = ArrayView::Contiguous { dims };
                    Some(ArraySource::Named(id.clone(), array_view))
                } else {
                    None
                };
                Expr2::Var(id, array_source, loc)
            }
            Expr1::App(builtin_fn, loc) => {
                use BuiltinFn::*;
                let builtin = match builtin_fn {
                    Lookup(v, e, loc) => Lookup(v, Box::new(Expr2::from(*e, ctx)?), loc),
                    Abs(e) => Abs(Box::new(Expr2::from(*e, ctx)?)),
                    Arccos(e) => Arccos(Box::new(Expr2::from(*e, ctx)?)),
                    Arcsin(e) => Arcsin(Box::new(Expr2::from(*e, ctx)?)),
                    Arctan(e) => Arctan(Box::new(Expr2::from(*e, ctx)?)),
                    Cos(e) => Cos(Box::new(Expr2::from(*e, ctx)?)),
                    Exp(e) => Exp(Box::new(Expr2::from(*e, ctx)?)),
                    Inf => Inf,
                    Int(e) => Int(Box::new(Expr2::from(*e, ctx)?)),
                    IsModuleInput(s, loc) => IsModuleInput(s, loc),
                    Ln(e) => Ln(Box::new(Expr2::from(*e, ctx)?)),
                    Log10(e) => Log10(Box::new(Expr2::from(*e, ctx)?)),
                    Max(e1, e2) => Max(
                        Box::new(Expr2::from(*e1, ctx)?),
                        e2.map(|e| Expr2::from(*e, ctx)).transpose()?.map(Box::new),
                    ),
                    Mean(exprs) => {
                        let exprs: EquationResult<Vec<Expr2>> =
                            exprs.into_iter().map(|e| Expr2::from(e, ctx)).collect();
                        Mean(exprs?)
                    }
                    Min(e1, e2) => Min(
                        Box::new(Expr2::from(*e1, ctx)?),
                        e2.map(|e| Expr2::from(*e, ctx)).transpose()?.map(Box::new),
                    ),
                    Pi => Pi,
                    Pulse(e1, e2, e3) => Pulse(
                        Box::new(Expr2::from(*e1, ctx)?),
                        Box::new(Expr2::from(*e2, ctx)?),
                        e3.map(|e| Expr2::from(*e, ctx)).transpose()?.map(Box::new),
                    ),
                    Ramp(e1, e2, e3) => Ramp(
                        Box::new(Expr2::from(*e1, ctx)?),
                        Box::new(Expr2::from(*e2, ctx)?),
                        e3.map(|e| Expr2::from(*e, ctx)).transpose()?.map(Box::new),
                    ),
                    SafeDiv(e1, e2, e3) => SafeDiv(
                        Box::new(Expr2::from(*e1, ctx)?),
                        Box::new(Expr2::from(*e2, ctx)?),
                        e3.map(|e| Expr2::from(*e, ctx)).transpose()?.map(Box::new),
                    ),
                    Sin(e) => Sin(Box::new(Expr2::from(*e, ctx)?)),
                    Sqrt(e) => Sqrt(Box::new(Expr2::from(*e, ctx)?)),
                    Step(e1, e2) => Step(
                        Box::new(Expr2::from(*e1, ctx)?),
                        Box::new(Expr2::from(*e2, ctx)?),
                    ),
                    Tan(e) => Tan(Box::new(Expr2::from(*e, ctx)?)),
                    Time => Time,
                    TimeStep => TimeStep,
                    StartTime => StartTime,
                    FinalTime => FinalTime,
                    Rank(e, opt) => Rank(
                        Box::new(Expr2::from(*e, ctx)?),
                        opt.map(|(e1, opt_e2)| {
                            Ok::<_, crate::common::EquationError>((
                                Box::new(Expr2::from(*e1, ctx)?),
                                opt_e2
                                    .map(|e2| Expr2::from(*e2, ctx))
                                    .transpose()?
                                    .map(Box::new),
                            ))
                        })
                        .transpose()?,
                    ),
                    Size(e) => Size(Box::new(Expr2::from(*e, ctx)?)),
                    Stddev(e) => Stddev(Box::new(Expr2::from(*e, ctx)?)),
                    Sum(e) => Sum(Box::new(Expr2::from(*e, ctx)?)),
                };
                // TODO: Handle array sources for builtin functions that return arrays
                Expr2::App(builtin, None, loc)
            }
            Expr1::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr2>> =
                    args.into_iter().map(|e| IndexExpr2::from(e, ctx)).collect();
                let args = args?;

                // Check if the subscripted variable is an array
                let array_source = if let Some(dims) = ctx.get_dimensions(&id) {
                    // Create the base array view
                    let base_view = ArrayView::Contiguous { dims };

                    // Apply subscript operation to get the resulting view
                    match base_view.subscript(&args) {
                        Ok(view) => Some(Self::allocate_temp_array(ctx, &view)),
                        Err(_) => None, // Invalid subscript, treat as scalar
                    }
                } else {
                    None // Scalar variable or unknown variable
                };

                Expr2::Subscript(id, args, array_source, loc)
            }
            Expr1::Op1(op, l, loc) => {
                let l_expr = Expr2::from(*l, ctx)?;

                // Compute array source for unary operations
                let array_source = match (&op, l_expr.get_array_source()) {
                    (UnaryOp::Transpose, Some(ArraySource::Named(_, view)))
                    | (UnaryOp::Transpose, Some(ArraySource::Temp(_, view))) => {
                        // Transpose creates a strided view, but we allocate contiguous storage
                        let transposed = view.transpose();
                        Some(Self::allocate_temp_array(ctx, &transposed))
                    }
                    (_, Some(ArraySource::Named(_, view)))
                    | (_, Some(ArraySource::Temp(_, view))) => {
                        // Other unary ops preserve array structure
                        Some(Self::allocate_temp_array(ctx, view))
                    }
                    _ => None,
                };

                Expr2::Op1(op, Box::new(l_expr), array_source, loc)
            }
            Expr1::Op2(op, l, r, loc) => {
                let l_expr = Expr2::from(*l, ctx)?;
                let r_expr = Expr2::from(*r, ctx)?;

                // Compute array source for binary operations
                let array_source = Self::unify_array_sources(
                    ctx,
                    l_expr.get_array_source(),
                    r_expr.get_array_source(),
                    loc,
                )?;

                Expr2::Op2(op, Box::new(l_expr), Box::new(r_expr), array_source, loc)
            }
            Expr1::If(cond, t, f, loc) => {
                let cond_expr = Expr2::from(*cond, ctx)?;
                let t_expr = Expr2::from(*t, ctx)?;
                let f_expr = Expr2::from(*f, ctx)?;

                // Compute array source for if expressions
                let array_source = Self::unify_array_sources(
                    ctx,
                    t_expr.get_array_source(),
                    f_expr.get_array_source(),
                    loc,
                )?;

                Expr2::If(
                    Box::new(cond_expr),
                    Box::new(t_expr),
                    Box::new(f_expr),
                    array_source,
                    loc,
                )
            }
        };
        Ok(expr)
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr2::Const(_, _, loc) => *loc,
            Expr2::Var(_, _, loc) => *loc,
            Expr2::App(_, _, loc) => *loc,
            Expr2::Subscript(_, _, _, loc) => *loc,
            Expr2::Op1(_, _, _, loc) => *loc,
            Expr2::Op2(_, _, _, _, loc) => *loc,
            Expr2::If(_, _, _, _, loc) => *loc,
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr2::Const(_s, _n, _loc) => None,
            Expr2::Var(v, _, loc) if v == ident => Some(*loc),
            Expr2::Var(_v, _, _loc) => None,
            Expr2::App(builtin, _, _loc) => {
                let mut loc: Option<Loc> = None;
                walk_builtin_expr(builtin, |contents| match contents {
                    BuiltinContents::Ident(id, id_loc) => {
                        if ident == id {
                            loc = Some(id_loc);
                        }
                    }
                    BuiltinContents::Expr(expr) => {
                        if loc.is_none() {
                            loc = expr.get_var_loc(ident);
                        }
                    }
                });
                loc
            }
            Expr2::Subscript(v, _args, _, loc) if v == ident => Some(*loc),
            Expr2::Subscript(_v, args, _, _loc) => {
                for arg in args {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr2::Op1(_op, l, _, _loc) => l.get_var_loc(ident),
            Expr2::Op2(_op, l, r, _, _loc) => {
                if let Some(loc) = l.get_var_loc(ident) {
                    return Some(loc);
                }
                r.get_var_loc(ident)
            }
            Expr2::If(c, t, f, _, _loc) => {
                if let Some(loc) = c.get_var_loc(ident) {
                    return Some(loc);
                }
                if let Some(loc) = t.get_var_loc(ident) {
                    return Some(loc);
                }
                f.get_var_loc(ident)
            }
        }
    }
}

/// Evaluate a constant expression to an integer value.
/// This is used for array subscripts which must be integer constants.
fn const_int_eval(ast: &Expr2) -> EquationResult<i32> {
    match ast {
        Expr2::Const(_, n, loc) => {
            if approx_eq!(f64, *n, n.round()) {
                Ok(n.round() as i32)
            } else {
                eqn_err!(ExpectedInteger, loc.start, loc.end)
            }
        }
        Expr2::Var(_, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr2::App(_, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr2::Subscript(_, _, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr2::Op1(op, expr, _, loc) => {
            let expr = const_int_eval(expr)?;
            let result = match op {
                UnaryOp::Positive => expr,
                UnaryOp::Negative => -expr,
                UnaryOp::Not => i32::from(expr == 0),
                UnaryOp::Transpose => {
                    // Transpose doesn't make sense for integer evaluation
                    return eqn_err!(ExpectedInteger, loc.start, loc.end);
                }
            };
            Ok(result)
        }
        Expr2::Op2(op, l, r, _, _) => {
            let l = const_int_eval(l)?;
            let r = const_int_eval(r)?;
            let result = match op {
                BinaryOp::Add => l + r,
                BinaryOp::Sub => l - r,
                BinaryOp::Exp => l.pow(r as u32),
                BinaryOp::Mul => l * r,
                BinaryOp::Div => {
                    if r == 0 {
                        0
                    } else {
                        l / r
                    }
                }
                BinaryOp::Mod => l % r,
                BinaryOp::Gt => (l > r) as i32,
                BinaryOp::Lt => (l < r) as i32,
                BinaryOp::Gte => (l >= r) as i32,
                BinaryOp::Lte => (l <= r) as i32,
                BinaryOp::Eq => (l == r) as i32,
                BinaryOp::Neq => (l != r) as i32,
                BinaryOp::And => ((l != 0) && (r != 0)) as i32,
                BinaryOp::Or => ((l != 0) || (r != 0)) as i32,
            };
            Ok(result)
        }
        Expr2::If(_, _, _, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create indexed dimensions for testing
    fn indexed_dims(sizes: &[u32]) -> Vec<Dimension> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, &size)| Dimension::Indexed(format!("dim{i}"), size))
            .collect()
    }

    // Common test context for Expr2Context
    struct TestContext {
        temp_counter: u32,
        dimensions: HashMap<String, Vec<Dimension>>,
    }

    impl TestContext {
        fn new() -> Self {
            Self {
                temp_counter: 0,
                dimensions: HashMap::new(),
            }
        }

        fn with_counter(temp_counter: u32) -> Self {
            Self {
                temp_counter,
                dimensions: HashMap::new(),
            }
        }
    }

    impl Expr2Context for TestContext {
        fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>> {
            self.dimensions.get(ident).cloned()
        }

        fn allocate_temp_id(&mut self) -> u32 {
            let id = self.temp_counter;
            self.temp_counter += 1;
            id
        }
    }

    // Helper to create a row slice view
    fn row_slice_view(cols: usize, row_idx: usize) -> ArrayView {
        ArrayView::Strided {
            dims: vec![StridedDimension {
                dimension: Dimension::Indexed("col".to_string(), cols as u32),
                stride: 1,
            }],
            offset: row_idx * cols,
        }
    }

    // Helper to create a column slice view
    fn col_slice_view(rows: usize, cols: usize, col_idx: usize) -> ArrayView {
        ArrayView::Strided {
            dims: vec![StridedDimension {
                dimension: Dimension::Indexed("row".to_string(), rows as u32),
                stride: cols as isize,
            }],
            offset: col_idx,
        }
    }

    #[test]
    fn test_array_iterators() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];

        struct TestCase {
            name: &'static str,
            view: ArrayView,
            expected: Vec<f64>,
            expected_size: usize,
        }

        let test_cases = vec![
            TestCase {
                name: "contiguous 3x4",
                view: ArrayView::Contiguous {
                    dims: indexed_dims(&[3, 4]),
                },
                expected: data.clone(),
                expected_size: 12,
            },
            TestCase {
                name: "row slice (second row)",
                view: row_slice_view(4, 1),
                expected: vec![5.0, 6.0, 7.0, 8.0],
                expected_size: 4,
            },
            TestCase {
                name: "column slice (second column)",
                view: col_slice_view(3, 4, 1),
                expected: vec![2.0, 6.0, 10.0],
                expected_size: 3,
            },
            TestCase {
                name: "2x2 subarray from middle",
                view: ArrayView::Strided {
                    dims: vec![
                        StridedDimension {
                            dimension: Dimension::Indexed("row".to_string(), 2),
                            stride: 4,
                        },
                        StridedDimension {
                            dimension: Dimension::Indexed("col".to_string(), 2),
                            stride: 1,
                        },
                    ],
                    offset: 5, // Start at element 6 (index 5)
                },
                expected: vec![6.0, 7.0, 10.0, 11.0],
                expected_size: 4,
            },
        ];

        for tc in test_cases {
            let values: Vec<f64> = tc.view.iter(&data).collect();
            assert_eq!(values, tc.expected, "Failed for {}", tc.name);
            assert_eq!(
                tc.view.size(),
                tc.expected_size,
                "Failed size for {}",
                tc.name
            );
        }
    }

    #[test]
    fn test_empty_array() {
        let data = vec![1.0, 2.0, 3.0];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[0, 3]),
        };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, Vec::<f64>::new());
        assert_eq!(view.size(), 0);
    }

    #[test]
    fn test_scalar() {
        let data = vec![42.0];
        let view = ArrayView::Contiguous { dims: vec![] };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, vec![42.0]);
        assert_eq!(view.size(), 1);
    }

    #[test]
    fn test_3d_array() {
        let data: Vec<f64> = (1..=24).map(|i| i as f64).collect();
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[2, 3, 4]),
        };

        assert_eq!(view.size(), 24);
        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values.len(), 24);
        assert_eq!(values[0], 1.0);
        assert_eq!(values[23], 24.0);
    }

    #[test]
    fn test_subscript_single_index() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };

        // Select second row (index 1), all columns
        let indices = vec![
            IndexExpr2::Expr(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            IndexExpr2::Wildcard(Loc::default()),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 1);
                assert_eq!(dims[0].dimension.len(), 4);
                assert_eq!(*offset, 4); // Skip first row
            }
            _ => panic!("Expected Strided view"),
        }

        let values: Vec<f64> = subscripted.iter(&data).collect();
        assert_eq!(values, vec![5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn test_subscript_wildcard() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };

        // Select all rows, second column
        let indices = vec![
            IndexExpr2::Wildcard(Loc::default()),
            IndexExpr2::Expr(Expr2::Const("1".to_string(), 1.0, Loc::default())),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 1);
                assert_eq!(dims[0].dimension.len(), 3);
                assert_eq!(dims[0].stride, 4); // Row stride
                assert_eq!(*offset, 1); // Start at second column
            }
            _ => panic!("Expected Strided view"),
        }

        let values: Vec<f64> = subscripted.iter(&data).collect();
        assert_eq!(values, vec![2.0, 6.0, 10.0]);
    }

    #[test]
    fn test_subscript_range() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };

        // Select rows 0:2, columns 1:3
        let indices = vec![
            IndexExpr2::Range(
                Expr2::Const("0".to_string(), 0.0, Loc::default()),
                Expr2::Const("2".to_string(), 2.0, Loc::default()),
                Loc::default(),
            ),
            IndexExpr2::Range(
                Expr2::Const("1".to_string(), 1.0, Loc::default()),
                Expr2::Const("3".to_string(), 3.0, Loc::default()),
                Loc::default(),
            ),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 2);
                assert_eq!(dims[0].dimension.len(), 2); // 2 rows
                assert_eq!(dims[1].dimension.len(), 2); // 2 columns
                assert_eq!(dims[0].stride, 4); // Row stride
                assert_eq!(dims[1].stride, 1); // Column stride
                assert_eq!(*offset, 1); // Start at [0,1]
            }
            _ => panic!("Expected Strided view"),
        }

        let values: Vec<f64> = subscripted.iter(&data).collect();
        assert_eq!(values, vec![2.0, 3.0, 6.0, 7.0]);
    }

    #[test]
    fn test_subscript_scalar() {
        let _data = [
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };

        // Select a single element [1, 2]
        let indices = vec![
            IndexExpr2::Expr(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            IndexExpr2::Expr(Expr2::Const("2".to_string(), 2.0, Loc::default())),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match subscripted {
            ArrayView::Contiguous { dims } => {
                assert_eq!(dims.len(), 0); // Scalar

                // For scalar, we'd need to handle offset differently
                // This test shows the structure is correct
            }
            _ => panic!("Expected Contiguous scalar view"),
        }
    }

    #[test]
    fn test_subscript_partial_not_allowed() {
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[2, 3, 4]),
        };

        // Attempt to select first element of first dimension only (partial indexing)
        let indices = vec![IndexExpr2::Expr(Expr2::Const(
            "0".to_string(),
            0.0,
            Loc::default(),
        ))];

        // This should fail because we need exactly 3 indices for a 3D array
        assert!(view.subscript(&indices).is_err());

        // Also test with 2 indices for a 3D array
        let indices = vec![
            IndexExpr2::Expr(Expr2::Const("0".to_string(), 0.0, Loc::default())),
            IndexExpr2::Expr(Expr2::Const("1".to_string(), 1.0, Loc::default())),
        ];
        assert!(view.subscript(&indices).is_err());
    }

    #[test]
    fn test_subscript_errors() {
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };

        // Too many indices
        let indices = vec![
            IndexExpr2::Expr(Expr2::Const("0".to_string(), 0.0, Loc::default())),
            IndexExpr2::Expr(Expr2::Const("0".to_string(), 0.0, Loc::default())),
            IndexExpr2::Expr(Expr2::Const("0".to_string(), 0.0, Loc::default())),
        ];
        assert!(view.subscript(&indices).is_err());

        // Index out of bounds
        let indices = vec![
            IndexExpr2::Expr(Expr2::Const("5".to_string(), 5.0, Loc::default())),
            IndexExpr2::Expr(Expr2::Const("0".to_string(), 0.0, Loc::default())),
        ];
        assert!(view.subscript(&indices).is_err());

        // Invalid range
        let indices = vec![
            IndexExpr2::Range(
                Expr2::Const("2".to_string(), 2.0, Loc::default()),
                Expr2::Const("1".to_string(), 1.0, Loc::default()),
                Loc::default(),
            ),
            IndexExpr2::Wildcard(Loc::default()),
        ];
        assert!(view.subscript(&indices).is_err());

        // Too few indices
        let indices = vec![IndexExpr2::Expr(Expr2::Const(
            "0".to_string(),
            0.0,
            Loc::default(),
        ))];
        assert!(view.subscript(&indices).is_err());
    }

    #[test]
    fn test_subscript_with_expression_index() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };

        // Select row using an expression: 1 + 0, all columns
        let one_plus_zero = Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr2::Const("0".to_string(), 0.0, Loc::default())),
            None,
            Loc::default(),
        );
        let indices = vec![
            IndexExpr2::Expr(one_plus_zero),
            IndexExpr2::Wildcard(Loc::default()),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        let values: Vec<f64> = subscripted.iter(&data).collect();
        assert_eq!(values, vec![5.0, 6.0, 7.0, 8.0]); // Second row
    }

    #[test]
    fn test_const_int_eval() {
        // Helper to create const expression
        fn const_expr(val: f64) -> Expr2 {
            Expr2::Const(val.to_string(), val, Loc::default())
        }

        // Test basic constants
        let const_cases = vec![
            (0.0, 0),
            (1.0, 1),
            (-1.0, -1),
            (42.0, 42),
            (3.0, 3), // Tests rounding
        ];

        for (val, expected) in const_cases {
            assert_eq!(expected, const_int_eval(&const_expr(val)).unwrap());
        }

        // Test error case
        assert!(const_int_eval(&const_expr(3.5)).is_err());
        assert!(const_int_eval(&Expr2::Var("foo".to_string(), None, Loc::default())).is_err());

        // Test unary operations
        let unary_cases = vec![
            (UnaryOp::Negative, 5, -5),
            (UnaryOp::Positive, 5, 5),
            (UnaryOp::Not, 0, 1),
            (UnaryOp::Not, 5, 0),
        ];

        for (op, input, expected) in unary_cases {
            let expr = Expr2::Op1(op, Box::new(const_expr(input as f64)), None, Loc::default());
            assert_eq!(expected, const_int_eval(&expr).unwrap());
        }

        // Test binary operations
        struct BinaryTestCase {
            op: BinaryOp,
            left: i32,
            right: i32,
            expected: i32,
        }

        let binary_cases = vec![
            BinaryTestCase {
                op: BinaryOp::Add,
                left: 2,
                right: 3,
                expected: 5,
            },
            BinaryTestCase {
                op: BinaryOp::Sub,
                left: 4,
                right: 1,
                expected: 3,
            },
            BinaryTestCase {
                op: BinaryOp::Mul,
                left: 3,
                right: 4,
                expected: 12,
            },
            BinaryTestCase {
                op: BinaryOp::Div,
                left: 7,
                right: 3,
                expected: 2,
            },
            BinaryTestCase {
                op: BinaryOp::Div,
                left: 7,
                right: 0,
                expected: 0,
            }, // div by zero
            BinaryTestCase {
                op: BinaryOp::Mod,
                left: 15,
                right: 7,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Exp,
                left: 3,
                right: 3,
                expected: 27,
            },
            BinaryTestCase {
                op: BinaryOp::Gt,
                left: 4,
                right: 2,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Lt,
                left: 2,
                right: 4,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Eq,
                left: 3,
                right: 3,
                expected: 1,
            },
            BinaryTestCase {
                op: BinaryOp::Neq,
                left: 3,
                right: 4,
                expected: 1,
            },
        ];

        for tc in binary_cases {
            let expr = Expr2::Op2(
                tc.op,
                Box::new(const_expr(tc.left as f64)),
                Box::new(const_expr(tc.right as f64)),
                None,
                Loc::default(),
            );
            assert_eq!(
                tc.expected,
                const_int_eval(&expr).unwrap(),
                "Failed for {:?} {} {}",
                tc.op,
                tc.left,
                tc.right
            );
        }

        // Test complex expression: (2 * 3) + 1 = 7
        let complex = Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Op2(
                BinaryOp::Mul,
                Box::new(const_expr(2.0)),
                Box::new(const_expr(3.0)),
                None,
                Loc::default(),
            )),
            Box::new(const_expr(1.0)),
            None,
            Loc::default(),
        );
        assert_eq!(7, const_int_eval(&complex).unwrap());
    }

    #[test]
    fn test_allocate_temp_array_contiguous() {
        // Test allocate_temp_array with already contiguous view
        let mut ctx = TestContext::new();
        let dims = indexed_dims(&[3, 4]);
        let contiguous_view = ArrayView::Contiguous { dims };

        let array_source = Expr2::allocate_temp_array(&mut ctx, &contiguous_view);

        match array_source {
            ArraySource::Temp(id, view) => {
                assert_eq!(id, 0);
                assert!(view.is_contiguous());
                assert_eq!(view.size(), 12); // 3 * 4
                assert_eq!(view.shape(), vec![3, 4]);

                // We can also check dimension details if needed
                let dims = view.dimensions();
                assert_eq!(dims.len(), 2);
                assert_eq!(dims[0].len(), 3);
                assert_eq!(dims[1].len(), 4);
            }
            _ => panic!("Expected temp array source"),
        }
    }

    #[test]
    fn test_allocate_temp_array_strided() {
        // Test allocate_temp_array with strided view
        let mut ctx = TestContext::with_counter(10);

        // Create a strided view (like after a transpose or slice)
        let strided_view = ArrayView::Strided {
            dims: vec![
                StridedDimension {
                    dimension: Dimension::Indexed("row".to_string(), 3),
                    stride: 4,
                },
                StridedDimension {
                    dimension: Dimension::Indexed("col".to_string(), 2),
                    stride: 1,
                },
            ],
            offset: 5,
        };

        let array_source = Expr2::allocate_temp_array(&mut ctx, &strided_view);

        match array_source {
            ArraySource::Temp(id, view) => {
                assert_eq!(id, 10);
                assert!(view.is_contiguous());
                assert_eq!(view.size(), 6); // 3 * 2
                assert_eq!(view.shape(), vec![3, 2]);

                // Check dimension names
                let dims = view.dimensions();
                assert_eq!(dims.len(), 2);
                assert_eq!(dims[0].name(), "row");
                assert_eq!(dims[0].len(), 3);
                assert_eq!(dims[1].name(), "col");
                assert_eq!(dims[1].len(), 2);
            }
            _ => panic!("Expected temp array source"),
        }
    }

    #[test]
    fn test_transpose_2d() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[2, 3]),
        };
        let transposed = view.transpose();

        // Transpose should create a non-contiguous view
        assert!(!transposed.is_contiguous());

        // Original: [[1, 2, 3], [4, 5, 6]]
        // Transposed: [[1, 4], [2, 5], [3, 6]]
        let expected = vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0];
        let values: Vec<f64> = transposed.iter(&data).collect();
        assert_eq!(values, expected);

        // Check that it's a strided view with reversed dimensions
        match transposed {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 2);
                assert_eq!(dims[0].dimension.len(), 3); // was column dimension
                assert_eq!(dims[1].dimension.len(), 2); // was row dimension
                assert_eq!(dims[0].stride, 1);
                assert_eq!(dims[1].stride, 3);
                assert_eq!(offset, 0);
            }
            _ => panic!("Expected Strided view after transpose"),
        }
    }

    #[test]
    fn test_transpose_3d() {
        let data: Vec<f64> = (1..=24).map(|i| i as f64).collect();
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[2, 3, 4]),
        };
        let transposed = view.transpose();

        // Verify a few specific values
        let values: Vec<f64> = transposed.iter(&data).collect();
        assert_eq!(values.len(), 24);

        // For a 3D array with shape [2, 3, 4], transpose reverses to [4, 3, 2]
        // Original layout in memory (row-major):
        // [0,0,0]=1, [0,0,1]=2, [0,0,2]=3, [0,0,3]=4,
        // [0,1,0]=5, [0,1,1]=6, [0,1,2]=7, [0,1,3]=8,
        // [0,2,0]=9, [0,2,1]=10, [0,2,2]=11, [0,2,3]=12,
        // [1,0,0]=13, [1,0,1]=14, [1,0,2]=15, [1,0,3]=16,
        // [1,1,0]=17, [1,1,1]=18, [1,1,2]=19, [1,1,3]=20,
        // [1,2,0]=21, [1,2,1]=22, [1,2,2]=23, [1,2,3]=24

        // After transpose, the new shape is [4, 3, 2] and the iteration order is:
        // [0,0,0]=1, [0,0,1]=13, [0,1,0]=5, [0,1,1]=17, [0,2,0]=9, [0,2,1]=21,
        // [1,0,0]=2, [1,0,1]=14, ...
        assert_eq!(values[0], 1.0); // [0,0,0] in transposed = [0,0,0] in original
        assert_eq!(values[1], 13.0); // [0,0,1] in transposed = [1,0,0] in original

        // Check dimensions are reversed
        match transposed {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 3);
                assert_eq!(dims[0].dimension.len(), 4); // was last dimension
                assert_eq!(dims[1].dimension.len(), 3); // was middle dimension
                assert_eq!(dims[2].dimension.len(), 2); // was first dimension
                assert_eq!(offset, 0);
            }
            _ => panic!("Expected Strided view after transpose"),
        }
    }

    #[test]
    fn test_is_contiguous() {
        // Test 1: Contiguous variant is always contiguous
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };
        assert!(view.is_contiguous());

        // Test 2: Strided with row-major strides and zero offset is contiguous
        let strided_contiguous = ArrayView::Strided {
            dims: vec![
                StridedDimension {
                    dimension: Dimension::Indexed("row".to_string(), 3),
                    stride: 4,
                },
                StridedDimension {
                    dimension: Dimension::Indexed("col".to_string(), 4),
                    stride: 1,
                },
            ],
            offset: 0,
        };
        assert!(strided_contiguous.is_contiguous());

        // Test 3: Non-zero offset makes it non-contiguous
        let strided_with_offset = ArrayView::Strided {
            dims: vec![
                StridedDimension {
                    dimension: Dimension::Indexed("row".to_string(), 3),
                    stride: 4,
                },
                StridedDimension {
                    dimension: Dimension::Indexed("col".to_string(), 4),
                    stride: 1,
                },
            ],
            offset: 5,
        };
        assert!(!strided_with_offset.is_contiguous());

        // Test 4: Wrong strides make it non-contiguous (e.g., after transpose)
        let strided_transposed = ArrayView::Strided {
            dims: vec![
                StridedDimension {
                    dimension: Dimension::Indexed("col".to_string(), 4),
                    stride: 1,
                },
                StridedDimension {
                    dimension: Dimension::Indexed("row".to_string(), 3),
                    stride: 4,
                },
            ],
            offset: 0,
        };
        assert!(!strided_transposed.is_contiguous());

        // Test 5: Scalar views are contiguous
        let scalar_contiguous = ArrayView::Contiguous { dims: vec![] };
        assert!(scalar_contiguous.is_contiguous());

        let scalar_strided = ArrayView::Strided {
            dims: vec![],
            offset: 0,
        };
        assert!(scalar_strided.is_contiguous());
    }

    #[test]
    fn test_transpose_strided() {
        // Test transposing an already strided view
        let strided_view = ArrayView::Strided {
            dims: vec![
                StridedDimension {
                    dimension: Dimension::Indexed("row".to_string(), 3),
                    stride: 4,
                },
                StridedDimension {
                    dimension: Dimension::Indexed("col".to_string(), 2),
                    stride: 1,
                },
            ],
            offset: 0,
        };

        let transposed = strided_view.transpose();

        match transposed {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 2);
                // Dimensions should be reversed
                assert_eq!(dims[0].dimension.name(), "col");
                assert_eq!(dims[0].dimension.len(), 2);
                assert_eq!(dims[0].stride, 1);
                assert_eq!(dims[1].dimension.name(), "row");
                assert_eq!(dims[1].dimension.len(), 3);
                assert_eq!(dims[1].stride, 4);
                assert_eq!(offset, 0);
            }
            _ => panic!("Expected Strided view"),
        }
    }

    #[test]
    fn test_subscript_with_dimension_name() {
        // Test Case 1: Using dimension name selects all elements
        let dims = vec![
            Dimension::Indexed("row".to_string(), 3),
            Dimension::Indexed("col".to_string(), 4),
        ];
        let view = ArrayView::Contiguous { dims };

        // Use "row" and "col" as subscripts - should select all elements
        let indices = vec![
            IndexExpr2::Expr(Expr2::Var("row".to_string(), None, Loc::default())),
            IndexExpr2::Expr(Expr2::Var("col".to_string(), None, Loc::default())),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 2);
                // Both dimensions should be preserved
                assert_eq!(dims[0].dimension.name(), "row");
                assert_eq!(dims[0].dimension.len(), 3);
                assert_eq!(dims[1].dimension.name(), "col");
                assert_eq!(dims[1].dimension.len(), 4);
                assert_eq!(*offset, 0);
            }
            _ => panic!("Expected Strided view"),
        }
    }

    #[test]
    fn test_subscript_with_named_element() {
        // Test Case 2: Using element name from a named dimension
        let named_dim = NamedDimension {
            elements: vec![
                "Boston".to_string(),
                "Chicago".to_string(),
                "LA".to_string(),
            ],
            indexed_elements: vec![
                ("Boston".to_string(), 1),
                ("Chicago".to_string(), 2),
                ("LA".to_string(), 3),
            ]
            .into_iter()
            .collect(),
        };
        let dims = vec![
            Dimension::Named("Location".to_string(), named_dim),
            Dimension::Indexed("Product".to_string(), 2),
        ];
        let view = ArrayView::Contiguous { dims };

        // Select "Chicago" for location and all products
        let indices = vec![
            IndexExpr2::Expr(Expr2::Var("Chicago".to_string(), None, Loc::default())),
            IndexExpr2::Wildcard(Loc::default()), // All products
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 1);
                // Only Product dimension should remain (Location dimension reduced)
                assert_eq!(dims[0].dimension.name(), "Product");
                assert_eq!(dims[0].dimension.len(), 2);
                // Offset should skip to Chicago row (index 1, stride 2)
                assert_eq!(*offset, 2); // 1 * 2
            }
            _ => panic!("Expected Strided view"),
        }
    }

    #[test]
    fn test_subscript_with_numeric_string() {
        // Test Case 2 variant: Using numeric string for indexed dimension
        let dims = vec![
            Dimension::Indexed("row".to_string(), 3),
            Dimension::Indexed("col".to_string(), 4),
        ];
        let view = ArrayView::Contiguous { dims };

        // Select row "2" (1-based, so index 1 in 0-based) and column "3"
        let indices = vec![
            IndexExpr2::Expr(Expr2::Var("2".to_string(), None, Loc::default())),
            IndexExpr2::Expr(Expr2::Var("3".to_string(), None, Loc::default())),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Contiguous { dims } => {
                assert_eq!(dims.len(), 0); // Scalar result
            }
            _ => panic!("Expected Contiguous scalar view"),
        }
    }

    #[test]
    fn test_subscript_with_unknown_variable() {
        // Test Case 3: Using unknown variable name (runtime index)
        let dims = vec![
            Dimension::Indexed("row".to_string(), 3),
            Dimension::Indexed("col".to_string(), 4),
        ];
        let view = ArrayView::Contiguous { dims };

        // Use "some_var" which is neither dimension name nor element
        let indices = vec![
            IndexExpr2::Expr(Expr2::Var("some_var".to_string(), None, Loc::default())),
            IndexExpr2::Expr(Expr2::Var("another_var".to_string(), None, Loc::default())),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Contiguous { dims } => {
                assert_eq!(dims.len(), 0); // Scalar result
            }
            _ => panic!("Expected Contiguous scalar view"),
        }
    }

    #[test]
    fn test_subscript_mixed_cases() {
        // Test mixing different subscript types
        let named_dim = NamedDimension {
            elements: vec!["A".to_string(), "B".to_string()],
            indexed_elements: vec![("A".to_string(), 1), ("B".to_string(), 2)]
                .into_iter()
                .collect(),
        };
        let dims = vec![
            Dimension::Named("Letter".to_string(), named_dim),
            Dimension::Indexed("Number".to_string(), 3),
            Dimension::Indexed("Color".to_string(), 2),
        ];
        let view = ArrayView::Contiguous { dims };

        // Mix of element name, dimension name, and wildcard
        let indices = vec![
            IndexExpr2::Expr(Expr2::Var("B".to_string(), None, Loc::default())), // Element "B"
            IndexExpr2::Expr(Expr2::Var("Number".to_string(), None, Loc::default())), // Dimension name
            IndexExpr2::Wildcard(Loc::default()),                                     // Wildcard
        ];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 2);
                // Number dimension preserved (dimension name used)
                assert_eq!(dims[0].dimension.name(), "Number");
                assert_eq!(dims[0].dimension.len(), 3);
                // Color dimension preserved (wildcard)
                assert_eq!(dims[1].dimension.name(), "Color");
                assert_eq!(dims[1].dimension.len(), 2);
                // Offset skips to "B" row: 1 * (3*2) = 6
                assert_eq!(*offset, 6);
            }
            _ => panic!("Expected Strided view"),
        }
    }

    #[test]
    fn test_subscript_with_data_iteration() {
        // Test that subscripting with variable names produces correct data
        let data = vec![
            1.0, 2.0, 3.0, 4.0, // row 0
            5.0, 6.0, 7.0, 8.0, // row 1
            9.0, 10.0, 11.0, 12.0, // row 2
        ];

        let named_dim = NamedDimension {
            elements: vec![
                "First".to_string(),
                "Second".to_string(),
                "Third".to_string(),
            ],
            indexed_elements: vec![
                ("First".to_string(), 1),
                ("Second".to_string(), 2),
                ("Third".to_string(), 3),
            ]
            .into_iter()
            .collect(),
        };
        let dims = vec![
            Dimension::Named("Row".to_string(), named_dim),
            Dimension::Indexed("Col".to_string(), 4),
        ];
        let view = ArrayView::Contiguous { dims };

        // Select "Second" row, all columns
        let indices = vec![
            IndexExpr2::Expr(Expr2::Var("Second".to_string(), None, Loc::default())),
            IndexExpr2::Wildcard(Loc::default()),
        ];
        let subscripted = view.subscript(&indices).unwrap();

        let values: Vec<f64> = subscripted.iter(&data).collect();
        assert_eq!(values, vec![5.0, 6.0, 7.0, 8.0]);
    }
}
