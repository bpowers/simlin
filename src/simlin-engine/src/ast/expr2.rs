// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{EquationResult, Ident};
use std::iter::Iterator;

#[derive(PartialEq, Clone, Debug)]
pub enum ArrayView {
    /// Simple contiguous array in row-major order
    Contiguous { shape: Vec<usize> },
    /// Strided array view (for transposes, slices, etc.)
    Strided {
        shape: Vec<usize>,
        strides: Vec<isize>, // One stride per dimension, isize for negative strides
        offset: usize,       // Starting offset into the underlying data
    },
}

impl ArrayView {
    /// Returns the total number of elements in the array
    pub fn size(&self) -> usize {
        match self {
            ArrayView::Contiguous { shape } | ArrayView::Strided { shape, .. } => {
                shape.iter().product()
            }
        }
    }

    /// Creates an iterator over the array elements in logical order
    pub fn iter<'a>(&self, data: &'a [f64]) -> ArrayIterator<'a> {
        match self {
            ArrayView::Contiguous { shape } => ArrayIterator::new_contiguous(data, shape),
            ArrayView::Strided {
                shape,
                strides,
                offset,
            } => ArrayIterator::new_strided(data, shape, strides, *offset),
        }
    }

    /// Creates a transposed view (reverses dimensions)
    pub fn transpose(&self) -> Self {
        match self {
            ArrayView::Contiguous { shape } => {
                if shape.is_empty() {
                    // Scalar case - transpose is identity
                    return ArrayView::Contiguous {
                        shape: shape.clone(),
                    };
                }

                let mut new_shape = shape.clone();
                new_shape.reverse();

                // Calculate strides for the original shape in row-major order
                let mut strides = vec![1isize; shape.len()];
                for i in (0..shape.len() - 1).rev() {
                    strides[i] = strides[i + 1] * shape[i + 1] as isize;
                }
                // Reverse strides for transpose
                strides.reverse();

                ArrayView::Strided {
                    shape: new_shape,
                    strides,
                    offset: 0,
                }
            }
            ArrayView::Strided {
                shape,
                strides,
                offset,
            } => {
                let mut new_shape = shape.clone();
                let mut new_strides = strides.clone();
                new_shape.reverse();
                new_strides.reverse();

                ArrayView::Strided {
                    shape: new_shape,
                    strides: new_strides,
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
    fn new_contiguous(data: &'a [f64], shape: &[usize]) -> Self {
        // Calculate strides for contiguous row-major array
        let mut strides = Vec::with_capacity(shape.len());
        let mut stride = 1isize;
        for &size in shape.iter().rev() {
            strides.push(stride);
            stride *= size as isize;
        }
        strides.reverse();

        let indices = vec![0; shape.len()];
        let done = shape.contains(&0); // Only mark done if any dimension is 0

        ArrayIterator {
            data,
            shape: shape.to_vec(),
            strides,
            offset: 0,
            indices,
            done,
        }
    }

    fn new_strided(data: &'a [f64], shape: &[usize], strides: &[isize], offset: usize) -> Self {
        let indices = vec![0; shape.len()];
        let done = shape.contains(&0); // Only mark done if any dimension is 0

        ArrayIterator {
            data,
            shape: shape.to_vec(),
            strides: strides.to_vec(),
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
    Expr(Expr2),
}

impl IndexExpr2 {
    pub(crate) fn from(expr: IndexExpr1) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr1::Wildcard(loc) => IndexExpr2::Wildcard(loc),
            IndexExpr1::StarRange(ident, loc) => IndexExpr2::StarRange(ident, loc),
            IndexExpr1::Range(l, r, loc) => {
                IndexExpr2::Range(Expr2::from(l)?, Expr2::from(r)?, loc)
            }
            IndexExpr1::Expr(e) => IndexExpr2::Expr(Expr2::from(e)?),
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

impl Expr2 {
    #[allow(dead_code)]
    pub(crate) fn from(expr: Expr1) -> EquationResult<Self> {
        let expr = match expr {
            Expr1::Const(s, n, loc) => Expr2::Const(s, n, loc),
            Expr1::Var(id, loc) => Expr2::Var(id, None, loc),
            Expr1::App(builtin_fn, loc) => {
                use BuiltinFn::*;
                let builtin = match builtin_fn {
                    Lookup(v, e, loc) => Lookup(v, Box::new(Expr2::from(*e)?), loc),
                    Abs(e) => Abs(Box::new(Expr2::from(*e)?)),
                    Arccos(e) => Arccos(Box::new(Expr2::from(*e)?)),
                    Arcsin(e) => Arcsin(Box::new(Expr2::from(*e)?)),
                    Arctan(e) => Arctan(Box::new(Expr2::from(*e)?)),
                    Cos(e) => Cos(Box::new(Expr2::from(*e)?)),
                    Exp(e) => Exp(Box::new(Expr2::from(*e)?)),
                    Inf => Inf,
                    Int(e) => Int(Box::new(Expr2::from(*e)?)),
                    IsModuleInput(s, loc) => IsModuleInput(s, loc),
                    Ln(e) => Ln(Box::new(Expr2::from(*e)?)),
                    Log10(e) => Log10(Box::new(Expr2::from(*e)?)),
                    Max(e1, e2) => Max(
                        Box::new(Expr2::from(*e1)?),
                        e2.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Mean(exprs) => {
                        let exprs: EquationResult<Vec<Expr2>> =
                            exprs.into_iter().map(Expr2::from).collect();
                        Mean(exprs?)
                    }
                    Min(e1, e2) => Min(
                        Box::new(Expr2::from(*e1)?),
                        e2.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Pi => Pi,
                    Pulse(e1, e2, e3) => Pulse(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Ramp(e1, e2, e3) => Ramp(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    SafeDiv(e1, e2, e3) => SafeDiv(
                        Box::new(Expr2::from(*e1)?),
                        Box::new(Expr2::from(*e2)?),
                        e3.map(|e| Expr2::from(*e)).transpose()?.map(Box::new),
                    ),
                    Sin(e) => Sin(Box::new(Expr2::from(*e)?)),
                    Sqrt(e) => Sqrt(Box::new(Expr2::from(*e)?)),
                    Step(e1, e2) => Step(Box::new(Expr2::from(*e1)?), Box::new(Expr2::from(*e2)?)),
                    Tan(e) => Tan(Box::new(Expr2::from(*e)?)),
                    Time => Time,
                    TimeStep => TimeStep,
                    StartTime => StartTime,
                    FinalTime => FinalTime,
                    Rank(e, opt) => Rank(
                        Box::new(Expr2::from(*e)?),
                        opt.map(|(e1, opt_e2)| {
                            Ok::<_, crate::common::EquationError>((
                                Box::new(Expr2::from(*e1)?),
                                opt_e2.map(|e2| Expr2::from(*e2)).transpose()?.map(Box::new),
                            ))
                        })
                        .transpose()?,
                    ),
                    Size(e) => Size(Box::new(Expr2::from(*e)?)),
                    Stddev(e) => Stddev(Box::new(Expr2::from(*e)?)),
                    Sum(e) => Sum(Box::new(Expr2::from(*e)?)),
                };
                Expr2::App(builtin, None, loc)
            }
            Expr1::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr2>> =
                    args.into_iter().map(IndexExpr2::from).collect();
                Expr2::Subscript(id, args?, None, loc)
            }
            Expr1::Op1(op, l, loc) => Expr2::Op1(op, Box::new(Expr2::from(*l)?), None, loc),
            Expr1::Op2(op, l, r, loc) => Expr2::Op2(
                op,
                Box::new(Expr2::from(*l)?),
                Box::new(Expr2::from(*r)?),
                None,
                loc,
            ),
            Expr1::If(cond, t, f, loc) => Expr2::If(
                Box::new(Expr2::from(*cond)?),
                Box::new(Expr2::from(*t)?),
                Box::new(Expr2::from(*f)?),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contiguous_iterator() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous { shape: vec![3, 4] };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, data);
        assert_eq!(view.size(), 12);
    }

    #[test]
    fn test_transpose() {
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0,
        ];
        let view = ArrayView::Contiguous { shape: vec![3, 4] };
        let transposed = view.transpose();

        // Original: [[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]]
        // Transposed: [[1, 5, 9], [2, 6, 10], [3, 7, 11], [4, 8, 12]]
        let expected = vec![
            1.0, 5.0, 9.0, 2.0, 6.0, 10.0, 3.0, 7.0, 11.0, 4.0, 8.0, 12.0,
        ];
        let values: Vec<f64> = transposed.iter(&data).collect();
        assert_eq!(values, expected);

        // Check shape
        match transposed {
            ArrayView::Strided { shape, .. } => assert_eq!(shape, vec![4, 3]),
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
            shape: vec![4],
            strides: vec![1],
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
            shape: vec![3],
            strides: vec![4], // Skip 4 elements to get to next row
            offset: 1,        // Start at second element
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
            shape: vec![2, 2],
            strides: vec![4, 1], // Same strides as original
            offset: 5,           // Start at element 6 (index 5)
        };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, vec![6.0, 7.0, 10.0, 11.0]);
    }

    #[test]
    fn test_empty_array() {
        let data = vec![1.0, 2.0, 3.0];
        let view = ArrayView::Contiguous { shape: vec![0, 3] };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, Vec::<f64>::new());
        assert_eq!(view.size(), 0);
    }

    #[test]
    fn test_scalar() {
        let data = vec![42.0];
        let view = ArrayView::Contiguous { shape: vec![] };

        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values, vec![42.0]);
        assert_eq!(view.size(), 1);
    }

    #[test]
    fn test_3d_array() {
        let data: Vec<f64> = (1..=24).map(|i| i as f64).collect();
        let view = ArrayView::Contiguous {
            shape: vec![2, 3, 4],
        };

        assert_eq!(view.size(), 24);
        let values: Vec<f64> = view.iter(&data).collect();
        assert_eq!(values.len(), 24);
        assert_eq!(values[0], 1.0);
        assert_eq!(values[23], 24.0);
    }
}
