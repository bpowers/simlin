// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{EquationResult, Ident};
use crate::dimensions::Dimension;
use crate::eqn_err;
use float_cmp::approx_eq;
use std::collections::HashMap;
use std::iter::Iterator;

/// Combines a dimension with its stride for strided arrays
#[derive(PartialEq, Clone, Debug)]
pub struct StridedDimension {
    pub dimension: Dimension,
    pub stride: isize,
}

#[derive(PartialEq, Clone, Debug)]
pub enum ArrayView {
    /// Simple contiguous array in row-major order
    Contiguous { dims: Vec<Dimension> },
    /// Strided array view (for transposes, slices, etc.)
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

                let mut new_dims = Vec::new();
                let mut offset = 0usize;

                for (i, index_expr) in indices.iter().enumerate() {
                    if i >= dims.len() {
                        // Get location from the first excess index
                        let loc = indices
                            .get(dims.len())
                            .map(|idx| idx.get_loc())
                            .unwrap_or_else(Loc::default);
                        return eqn_err!(Generic, loc.start, loc.end);
                    }

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
                            // Evaluate the expression to get an integer index
                            let idx = const_int_eval(expr)?;

                            if idx < 0 || idx as usize >= dims[i].len() {
                                return eqn_err!(Generic, loc.start, loc.end);
                            }
                            // Skip this dimension and adjust offset
                            offset += idx as usize * original_strides[i] as usize;
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
                                        crate::dimensions::NamedDimension {
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
                if new_dims.is_empty() && indices.len() == dims.len() {
                    // This represents a scalar - could return Contiguous with empty dims
                    Ok(ArrayView::Contiguous { dims: vec![] })
                } else if new_dims.is_empty() {
                    // Partial indexing - keep remaining dimensions
                    let mut remaining_dims = Vec::new();
                    for i in indices.len()..dims.len() {
                        remaining_dims.push(StridedDimension {
                            dimension: dims[i].clone(),
                            stride: original_strides[i],
                        });
                    }
                    Ok(ArrayView::Strided {
                        dims: remaining_dims,
                        offset,
                    })
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
                        Ok(view) => Some(ArraySource::Temp(ctx.allocate_temp_id(), view)),
                        Err(_) => None, // Invalid subscript, treat as scalar
                    }
                } else {
                    None // Scalar variable or unknown variable
                };

                Expr2::Subscript(id, args, array_source, loc)
            }
            Expr1::Op1(op, l, loc) => Expr2::Op1(op, Box::new(Expr2::from(*l, ctx)?), None, loc),
            Expr1::Op2(op, l, r, loc) => Expr2::Op2(
                op,
                Box::new(Expr2::from(*l, ctx)?),
                Box::new(Expr2::from(*r, ctx)?),
                None,
                loc,
            ),
            Expr1::If(cond, t, f, loc) => Expr2::If(
                Box::new(Expr2::from(*cond, ctx)?),
                Box::new(Expr2::from(*t, ctx)?),
                Box::new(Expr2::from(*f, ctx)?),
                None,
                loc,
            ),
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

    #[test]
    fn test_contiguous_iterator() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, data);
        assert_eq!(view.size(), 12);
    }

    #[test]
    fn test_transpose() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[3, 4]),
        };
        let transposed = view.transpose();

        // Original: [[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]]
        // Transposed: [[1, 5, 9], [2, 6, 10], [3, 7, 11], [4, 8, 12]]
        let expected = vec![
            1.0, 5.0, 9.0, 2.0, 6.0, 10.0, 3.0, 7.0, 11.0, 4.0, 8.0, 12.0,
        ];
        let values: Vec<f64> = transposed.iter(&data).collect();
        assert_eq!(values, expected);

        // Check that it's a strided view
        match transposed {
            ArrayView::Strided { .. } => (),
            _ => panic!("Expected Strided view after transpose"),
        }
    }

    #[test]
    fn test_row_slice() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        // Select row 1 (second row): [5, 6, 7, 8]
        let view = ArrayView::Strided {
            dims: vec![StridedDimension {
                dimension: Dimension::Indexed("col".to_string(), 4),
                stride: 1,
            }],
            offset: 4, // Skip first row (4 elements)
        };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, vec![5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn test_column_slice() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        // Select column 1 (second column): [2, 6, 10]
        let view = ArrayView::Strided {
            dims: vec![StridedDimension {
                dimension: Dimension::Indexed("row".to_string(), 3),
                stride: 4, // Skip 4 elements to get to next row
            }],
            offset: 1, // Start at second element
        };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, vec![2.0, 6.0, 10.0]);
    }

    #[test]
    fn test_subarray() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        // Select a 2x2 subarray from the middle: [[6, 7], [10, 11]]
        let view = ArrayView::Strided {
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
        };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, vec![6.0, 7.0, 10.0, 11.0]);
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

        // Select second row (index 1)
        let indices = vec![IndexExpr2::Expr(Expr2::Const(
            "1".to_string(),
            1.0,
            Loc::default(),
        ))];
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
    fn test_subscript_partial() {
        let data: Vec<f64> = (1..=24).map(|i| i as f64).collect();
        let view = ArrayView::Contiguous {
            dims: indexed_dims(&[2, 3, 4]),
        };

        // Select first element of first dimension only
        let indices = vec![IndexExpr2::Expr(Expr2::Const(
            "0".to_string(),
            0.0,
            Loc::default(),
        ))];
        let subscripted = view.subscript(&indices).unwrap();

        match &subscripted {
            ArrayView::Strided { dims, offset } => {
                assert_eq!(dims.len(), 2); // Remaining dimensions
                assert_eq!(dims[0].dimension.len(), 3);
                assert_eq!(dims[1].dimension.len(), 4);
                assert_eq!(*offset, 0); // First slice
            }
            _ => panic!("Expected Strided view"),
        }

        let values: Vec<f64> = subscripted.iter(&data).collect();
        assert_eq!(values.len(), 12);
        assert_eq!(values[0], 1.0);
        assert_eq!(values[11], 12.0);
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
        let indices = vec![IndexExpr2::Expr(Expr2::Const(
            "5".to_string(),
            5.0,
            Loc::default(),
        ))];
        assert!(view.subscript(&indices).is_err());

        // Invalid range
        let indices = vec![IndexExpr2::Range(
            Expr2::Const("2".to_string(), 2.0, Loc::default()),
            Expr2::Const("1".to_string(), 1.0, Loc::default()),
            Loc::default(),
        )];
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

        // Select row using an expression: 1 + 0
        let one_plus_zero = Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr2::Const("0".to_string(), 0.0, Loc::default())),
            None,
            Loc::default(),
        );
        let indices = vec![IndexExpr2::Expr(one_plus_zero)];
        let subscripted = view.subscript(&indices).unwrap();

        let values: Vec<f64> = subscripted.iter(&data).collect();
        assert_eq!(values, vec![5.0, 6.0, 7.0, 8.0]); // Second row
    }

    #[test]
    fn test_const_int_eval() {
        // Basic constants
        assert_eq!(
            0,
            const_int_eval(&Expr2::Const("0".to_string(), 0.0, Loc::default())).unwrap()
        );
        assert_eq!(
            1,
            const_int_eval(&Expr2::Const("1".to_string(), 1.0, Loc::default())).unwrap()
        );
        assert_eq!(
            -1,
            const_int_eval(&Expr2::Const("-1".to_string(), -1.0, Loc::default())).unwrap()
        );
        assert_eq!(
            42,
            const_int_eval(&Expr2::Const("42".to_string(), 42.0, Loc::default())).unwrap()
        );

        // Rounds correctly
        assert_eq!(
            3,
            const_int_eval(&Expr2::Const("3.0".to_string(), 3.0, Loc::default())).unwrap()
        );

        // Unary operations
        let neg_five = Expr2::Op1(
            UnaryOp::Negative,
            Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(-5, const_int_eval(&neg_five).unwrap());

        // Binary operations
        let two_plus_three = Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
            Box::new(Expr2::Const("3".to_string(), 3.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(5, const_int_eval(&two_plus_three).unwrap());

        let four_minus_one = Expr2::Op2(
            BinaryOp::Sub,
            Box::new(Expr2::Const("4".to_string(), 4.0, Loc::default())),
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(3, const_int_eval(&four_minus_one).unwrap());

        // Division truncates
        let two_div_three = Expr2::Op2(
            BinaryOp::Div,
            Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
            Box::new(Expr2::Const("3".to_string(), 3.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(0, const_int_eval(&two_div_three).unwrap());

        // Division by zero returns 0
        let seven_div_zero = Expr2::Op2(
            BinaryOp::Div,
            Box::new(Expr2::Const("7".to_string(), 7.0, Loc::default())),
            Box::new(Expr2::Const("0".to_string(), 0.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(0, const_int_eval(&seven_div_zero).unwrap());

        // Modulo
        let fifteen_mod_seven = Expr2::Op2(
            BinaryOp::Mod,
            Box::new(Expr2::Const("15".to_string(), 15.0, Loc::default())),
            Box::new(Expr2::Const("7".to_string(), 7.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(1, const_int_eval(&fifteen_mod_seven).unwrap());

        // Exponentiation
        let three_pow_three = Expr2::Op2(
            BinaryOp::Exp,
            Box::new(Expr2::Const("3".to_string(), 3.0, Loc::default())),
            Box::new(Expr2::Const("3".to_string(), 3.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(27, const_int_eval(&three_pow_three).unwrap());

        // Comparison operators
        let four_gt_two = Expr2::Op2(
            BinaryOp::Gt,
            Box::new(Expr2::Const("4".to_string(), 4.0, Loc::default())),
            Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(1, const_int_eval(&four_gt_two).unwrap());

        // Error cases
        assert!(const_int_eval(&Expr2::Const("3.5".to_string(), 3.5, Loc::default())).is_err());
        assert!(const_int_eval(&Expr2::Var("foo".to_string(), None, Loc::default())).is_err());

        // Complex expression
        let complex = Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Op2(
                BinaryOp::Mul,
                Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
                Box::new(Expr2::Const("3".to_string(), 3.0, Loc::default())),
                None,
                Loc::default(),
            )),
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            None,
            Loc::default(),
        );
        assert_eq!(7, const_int_eval(&complex).unwrap());
    }
}
