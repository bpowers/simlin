// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use super::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{EquationResult, Ident};
use crate::datamodel::Dimension;
use crate::eqn_err;

/// Specification for how to slice dimensions
#[derive(Clone, Debug, PartialEq)]
pub enum SliceSpec {
    /// Single element selection at given index
    Index(usize),
    /// Keep dimension (*)
    Wildcard,
    /// Range selection (start:end)
    Range(usize, usize),
    /// Dimension placeholder by name
    DimName(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DimensionRange {
    dim: Dimension,
    start: u32,
    end: u32,
}

impl DimensionRange {
    pub fn new(dim: Dimension, start: u32, end: u32) -> Self {
        DimensionRange { dim, start, end }
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

impl DimensionVec {
    /// Create dimension info from a vector of dimensions
    pub fn new(dims: Vec<DimensionRange>) -> Self {
        DimensionVec { dims }
    }

    /// Create dimension info for a scalar value (no dimensions)
    pub fn scalar() -> Self {
        DimensionVec { dims: vec![] }
    }

    /// Check if this represents a scalar value
    pub fn is_scalar(&self) -> bool {
        self.dims.is_empty()
    }

    /// Get the dimensions
    pub fn dimensions(&self) -> &[DimensionRange] {
        &self.dims
    }

    /// Get the number of dimensions
    pub fn ndim(&self) -> usize {
        self.dims.len()
    }

    /// Get the total number of elements
    pub fn size(&self) -> u32 {
        if self.is_scalar() {
            1
        } else {
            self.dims.iter().map(|d| d.len()).product()
        }
    }

    /// Get the shape as a vector of sizes
    pub fn shape(&self) -> Vec<u32> {
        self.dims.iter().map(|d| d.len()).collect()
    }

    /// Get dimension names
    pub fn names(&self) -> Vec<&str> {
        self.dims.iter().map(|d| d.dim.name()).collect()
    }

    /// Create new DimensionInfo with a subset of dimensions (for slicing)
    pub fn slice(&self, keep_dims: &[bool]) -> Self {
        assert_eq!(keep_dims.len(), self.dims.len());
        DimensionVec {
            dims: self
                .dims
                .iter()
                .zip(keep_dims.iter())
                .filter_map(|(dim, &keep)| if keep { Some(dim.clone()) } else { None })
                .collect(),
        }
    }

    /// Check if dimensions are compatible for element-wise operations
    pub fn is_compatible(&self, other: &Self) -> bool {
        self.dims == other.dims
    }

    /// Check if dimensions are broadcastable for element-wise operations
    /// Following NumPy-style broadcasting rules adapted for XMILE:
    /// 1. Scalar (0-dimensional) values can broadcast with any array
    /// 2. Dimensions are compared from right to left
    /// 3. Named dimensions must match by name, not just size
    pub fn is_broadcast_compatible(&self, other: &Self) -> bool {
        // Scalars are always broadcast compatible
        if self.is_scalar() || other.is_scalar() {
            return true;
        }

        // Compare dimensions from right to left
        let self_dims = &self.dims;
        let other_dims = &other.dims;

        let min_dims = self_dims.len().min(other_dims.len());

        // Check dimensions from right to left
        for i in 0..min_dims {
            let self_idx = self_dims.len() - 1 - i;
            let other_idx = other_dims.len() - 1 - i;

            let self_dim = &self_dims[self_idx];
            let other_dim = &other_dims[other_idx];

            // For named dimensions, names must match
            if self_dim.dim.name() != other_dim.dim.name() {
                return false;
            }

            // Sizes must be equal or one must be 1 (singleton)
            let self_size = self_dim.len();
            let other_size = other_dim.len();
            if self_size != other_size && self_size != 1 && other_size != 1 {
                return false;
            }
        }

        true
    }

    /// Result dimensions after broadcasting
    /// Returns Ok(dimensions) if broadcast is possible, Err otherwise
    pub fn broadcast_shape(&self, other: &Self) -> Result<Self, String> {
        if !self.is_broadcast_compatible(other) {
            return Err(format!(
                "Cannot broadcast dimensions {:?} with {:?}",
                self.names(),
                other.names()
            ));
        }

        // Handle scalar cases
        if self.is_scalar() {
            return Ok(other.clone());
        }
        if other.is_scalar() {
            return Ok(self.clone());
        }

        // Build result dimensions
        let self_dims = &self.dims;
        let other_dims = &other.dims;
        let max_dims = self_dims.len().max(other_dims.len());
        let mut result_dims = Vec::with_capacity(max_dims);

        // Add dimensions from the array with more dimensions
        let (longer, shorter) = if self_dims.len() >= other_dims.len() {
            (self_dims, other_dims)
        } else {
            (other_dims, self_dims)
        };

        // Add leading dimensions from longer array
        let lead_dims = longer.len() - shorter.len();
        for i in 0..lead_dims {
            result_dims.push(longer[i].clone());
        }

        // Process aligned dimensions
        for i in 0..shorter.len() {
            let long_idx = lead_dims + i;
            let short_idx = i;

            let long_dim = &longer[long_idx];
            let short_dim = &shorter[short_idx];

            // Choose the non-singleton dimension, or the larger if both are non-singleton
            let result_dim = if short_dim.len() == 1 {
                long_dim.clone()
            } else if long_dim.len() == 1 {
                short_dim.clone()
            } else {
                // Both are the same size (checked in is_broadcast_compatible)
                long_dim.clone()
            };

            result_dims.push(result_dim);
        }

        Ok(DimensionVec::new(result_dims))
    }

    /// Check if this can be assigned to target dimensions
    /// Assignment is valid if:
    /// 1. Dimensions are exactly equal, or
    /// 2. Source is broadcastable to target AND target has at least as many dimensions
    /// Note: Arrays cannot be assigned to scalars
    pub fn is_assignable_to(&self, target: &Self) -> bool {
        if self == target {
            return true;
        }

        // Check if broadcastable and that we're not trying to assign array to scalar
        if self.is_broadcast_compatible(target) {
            // If target is scalar, only scalar can be assigned to it
            if target.is_scalar() {
                return self.is_scalar();
            }
            // Otherwise, broadcasting rules apply
            return true;
        }

        false
    }

    /// Apply slicing operation using SliceSpec
    /// This replaces the simple boolean-based slice method with a more flexible approach
    pub fn slice_with_spec(&self, slice_specs: &[SliceSpec]) -> Result<Self, String> {
        if slice_specs.len() != self.dims.len() {
            return Err(format!(
                "Slice spec length {} doesn't match dimension count {}",
                slice_specs.len(),
                self.dims.len()
            ));
        }

        let mut result_dims = Vec::new();

        for (dim_range, spec) in self.dims.iter().zip(slice_specs.iter()) {
            match spec {
                SliceSpec::Index(_idx) => {
                    // Single index selection removes this dimension
                    // Note: actual bounds checking would happen during evaluation
                    continue;
                }
                SliceSpec::Wildcard => {
                    // Keep entire dimension
                    result_dims.push(dim_range.clone());
                }
                SliceSpec::Range(start, end) => {
                    // Create new dimension range with adjusted bounds
                    let new_range =
                        DimensionRange::new(dim_range.dim.clone(), *start as u32, *end as u32);
                    result_dims.push(new_range);
                }
                SliceSpec::DimName(name) => {
                    // For dimension placeholders, keep the dimension if name matches
                    if dim_range.dim.name() == name {
                        result_dims.push(dim_range.clone());
                    } else {
                        return Err(format!(
                            "Dimension name '{}' doesn't match dimension '{}'",
                            name,
                            dim_range.dim.name()
                        ));
                    }
                }
            }
        }

        Ok(DimensionVec::new(result_dims))
    }

    /// Transpose (reverse) dimensions
    /// This implements the XMILE transpose operator (') which reverses all dimensions
    pub fn transpose(&self) -> Self {
        let mut dims = self.dims.clone();
        dims.reverse();
        DimensionVec::new(dims)
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Exp,
    Mul,
    Div,
    Mod,
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

impl BinaryOp {
    /// higher the precedence, the tighter the binding.
    /// e.g. Mul.precedence() > Add.precedence()
    pub(crate) fn precedence(&self) -> u8 {
        // matches equation.lalrpop
        match self {
            BinaryOp::Add => 4,
            BinaryOp::Sub => 4,
            BinaryOp::Exp => 6,
            BinaryOp::Mul => 5,
            BinaryOp::Div => 5,
            BinaryOp::Mod => 5,
            BinaryOp::Gt => 3,
            BinaryOp::Lt => 3,
            BinaryOp::Gte => 3,
            BinaryOp::Lte => 3,
            BinaryOp::Eq => 2,
            BinaryOp::Neq => 2,
            BinaryOp::And => 1,
            BinaryOp::Or => 1,
        }
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
    Transpose,
}

/// Expr represents a dimension-annotated expression, the final stage
/// of AST transformation with full dimension information.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr2 {
    Const(String, f64, DimensionVec, Loc),
    Var(Ident, DimensionVec, Loc),
    App(BuiltinFn<Expr2>, DimensionVec, Loc),
    Subscript(Ident, Vec<IndexExpr2>, DimensionVec, Loc),
    Op1(UnaryOp, Box<Expr2>, DimensionVec, Loc),
    Op2(BinaryOp, Box<Expr2>, Box<Expr2>, DimensionVec, Loc),
    If(Box<Expr2>, Box<Expr2>, Box<Expr2>, DimensionVec, Loc),
}

/// IndexExpr represents a dimension-annotated index expression.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr2 {
    Wildcard(DimensionVec, Loc),
    StarRange(Ident, DimensionVec, Loc),
    Range(Expr2, Expr2, DimensionVec, Loc),
    Expr(Expr2),
}

impl Expr2 {
    /// Get the dimensions of this expression
    pub fn dims(&self) -> &DimensionVec {
        match self {
            Expr2::Const(_, _, dims, _) => dims,
            Expr2::Var(_, dims, _) => dims,
            Expr2::App(_, dims, _) => dims,
            Expr2::Subscript(_, _, dims, _) => dims,
            Expr2::Op1(_, _, dims, _) => dims,
            Expr2::Op2(_, _, _, dims, _) => dims,
            Expr2::If(_, _, _, dims, _) => dims,
        }
    }

    /// Get the location of this expression
    pub fn loc(&self) -> Loc {
        match self {
            Expr2::Const(_, _, _, loc) => *loc,
            Expr2::Var(_, _, loc) => *loc,
            Expr2::App(_, _, loc) => *loc,
            Expr2::Subscript(_, _, _, loc) => *loc,
            Expr2::Op1(_, _, _, loc) => *loc,
            Expr2::Op2(_, _, _, _, loc) => *loc,
            Expr2::If(_, _, _, _, loc) => *loc,
        }
    }
}

impl IndexExpr2 {
    /// Get the dimensions of this index expression
    pub fn dims(&self) -> &DimensionVec {
        match self {
            IndexExpr2::Wildcard(dims, _) => dims,
            IndexExpr2::StarRange(_, dims, _) => dims,
            IndexExpr2::Range(_, _, dims, _) => dims,
            IndexExpr2::Expr(expr) => expr.dims(),
        }
    }
}

/// Context for dimension inference
pub struct DimensionContext<'a> {
    /// Variable dimensions from the model
    pub var_dims: &'a HashMap<Ident, DimensionVec>,
}

impl IndexExpr2 {
    /// Infer dimensions for index expressions
    fn infer_dimensions(expr: IndexExpr1, ctx: &DimensionContext) -> EquationResult<Self> {
        match expr {
            IndexExpr1::Wildcard(loc) => Ok(IndexExpr2::Wildcard(DimensionVec::scalar(), loc)),
            IndexExpr1::StarRange(ident, loc) => {
                Ok(IndexExpr2::StarRange(ident, DimensionVec::scalar(), loc))
            }
            IndexExpr1::Range(start, end, loc) => {
                let start = Expr2::infer_dimensions(start, ctx)?;
                let end = Expr2::infer_dimensions(end, ctx)?;
                Ok(IndexExpr2::Range(start, end, DimensionVec::scalar(), loc))
            }
            IndexExpr1::Expr(e) => {
                let e = Expr2::infer_dimensions(e, ctx)?;
                Ok(IndexExpr2::Expr(e))
            }
        }
    }
}

impl Expr2 {
    /// Infer dimensions from an Expr1 to create a dimension-annotated Expr
    pub fn infer_dimensions(expr: Expr1, ctx: &DimensionContext) -> EquationResult<Self> {
        let result = match expr {
            Expr1::Const(s, n, loc) => {
                // Constants are always scalar
                Expr2::Const(s, n, DimensionVec::scalar(), loc)
            }
            Expr1::Var(id, loc) => {
                // Look up variable dimensions from context
                let dims = ctx
                    .var_dims
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(DimensionVec::scalar);
                Expr2::Var(id, dims, loc)
            }
            Expr1::App(builtin, loc) => {
                // Infer dimensions for builtin functions
                let (builtin, dims) = Self::infer_builtin_dimensions(builtin, ctx)?;
                Expr2::App(builtin, dims, loc)
            }
            Expr1::Subscript(id, indices, loc) => {
                // Get base variable dimensions
                let base_dims = ctx
                    .var_dims
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(DimensionVec::scalar);

                // Convert index expressions
                let index_exprs: Result<Vec<_>, _> = indices
                    .into_iter()
                    .map(|idx| IndexExpr2::infer_dimensions(idx, ctx))
                    .collect();
                let index_exprs = index_exprs?;

                // Calculate resulting dimensions after subscripting
                let result_dims = Self::apply_subscript_to_dims(&base_dims, &index_exprs, loc)?;

                Expr2::Subscript(id, index_exprs, result_dims, loc)
            }
            Expr1::Op1(op, expr, loc) => {
                let expr = Box::new(Self::infer_dimensions(*expr, ctx)?);
                let dims = match op {
                    UnaryOp::Transpose => expr.dims().transpose(),
                    _ => expr.dims().clone(), // Other unary ops preserve dimensions
                };
                Expr2::Op1(op, expr, dims, loc)
            }
            Expr1::Op2(op, l, r, loc) => {
                let l = Box::new(Self::infer_dimensions(*l, ctx)?);
                let r = Box::new(Self::infer_dimensions(*r, ctx)?);

                // Infer dimensions based on operation type
                let dims = Self::infer_binary_op_dims(op, l.dims(), r.dims(), loc)?;

                Expr2::Op2(op, l, r, dims, loc)
            }
            Expr1::If(cond, t, f, loc) => {
                let cond = Box::new(Self::infer_dimensions(*cond, ctx)?);
                let t = Box::new(Self::infer_dimensions(*t, ctx)?);
                let f = Box::new(Self::infer_dimensions(*f, ctx)?);

                // Condition should be scalar or broadcastable
                // Result has dimensions from broadcasting t and f
                let dims = match t.dims().broadcast_shape(f.dims()) {
                    Ok(dims) => dims,
                    Err(_) => return eqn_err!(MismatchedDimensions, loc.start, loc.end),
                };

                Expr2::If(cond, t, f, dims, loc)
            }
        };

        Ok(result)
    }

    /// Infer dimensions for builtin functions
    fn infer_builtin_dimensions(
        builtin: BuiltinFn<Expr1>,
        ctx: &DimensionContext,
    ) -> EquationResult<(BuiltinFn<Expr2>, DimensionVec)> {
        use BuiltinFn::*;

        match builtin {
            // Zero-argument functions return scalars
            Inf => Ok((Inf, DimensionVec::scalar())),
            Pi => Ok((Pi, DimensionVec::scalar())),
            Time => Ok((Time, DimensionVec::scalar())),
            TimeStep => Ok((TimeStep, DimensionVec::scalar())),
            StartTime => Ok((StartTime, DimensionVec::scalar())),
            FinalTime => Ok((FinalTime, DimensionVec::scalar())),

            // Single-argument functions that preserve dimensions
            Abs(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Abs(a), dims))
            }
            Arccos(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Arccos(a), dims))
            }
            Arcsin(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Arcsin(a), dims))
            }
            Arctan(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Arctan(a), dims))
            }
            Cos(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Cos(a), dims))
            }
            Exp(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Exp(a), dims))
            }
            Int(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Int(a), dims))
            }
            Ln(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Ln(a), dims))
            }
            Log10(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Log10(a), dims))
            }
            Sin(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Sin(a), dims))
            }
            Sqrt(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Sqrt(a), dims))
            }
            Tan(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Tan(a), dims))
            }

            // Array reduction functions return scalars
            Mean(args) => {
                let args: Result<Vec<_>, _> = args
                    .into_iter()
                    .map(|arg| Expr2::infer_dimensions(arg, ctx))
                    .collect();
                Ok((Mean(args?), DimensionVec::scalar()))
            }
            Sum(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                Ok((Sum(a), DimensionVec::scalar()))
            }
            Size(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                Ok((Size(a), DimensionVec::scalar()))
            }
            Stddev(a) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                Ok((Stddev(a), DimensionVec::scalar()))
            }

            // Min/Max with multiple arguments - result has broadcast shape
            Min(a, b_opt) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let mut dims = a.dims().clone();

                if let Some(b) = b_opt {
                    let b = Box::new(Expr2::infer_dimensions(*b, ctx)?);
                    dims = match dims.broadcast_shape(b.dims()) {
                        Ok(d) => d,
                        Err(_) => return eqn_err!(MismatchedDimensions, 0, 0),
                    };
                    Ok((Min(a, Some(b)), dims))
                } else {
                    Ok((Min(a, None), dims))
                }
            }
            Max(a, b_opt) => {
                // Same logic as Min
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let mut dims = a.dims().clone();

                if let Some(b) = b_opt {
                    let b = Box::new(Expr2::infer_dimensions(*b, ctx)?);
                    dims = match dims.broadcast_shape(b.dims()) {
                        Ok(d) => d,
                        Err(_) => return eqn_err!(MismatchedDimensions, 0, 0),
                    };
                    Ok((Max(a, Some(b)), dims))
                } else {
                    Ok((Max(a, None), dims))
                }
            }

            // Other builtins preserve first argument's dimensions or are scalar
            Lookup(id, a, loc) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                Ok((Lookup(id, a, loc), DimensionVec::scalar()))
            }
            IsModuleInput(id, loc) => Ok((IsModuleInput(id, loc), DimensionVec::scalar())),
            Pulse(a, b, c) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr2::infer_dimensions(*b, ctx)?);
                let c = match c {
                    Some(c_expr) => Some(Box::new(Expr2::infer_dimensions(*c_expr, ctx)?)),
                    None => None,
                };
                Ok((Pulse(a, b, c), DimensionVec::scalar()))
            }
            Ramp(a, b, c) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr2::infer_dimensions(*b, ctx)?);
                let c = match c {
                    Some(c_expr) => Some(Box::new(Expr2::infer_dimensions(*c_expr, ctx)?)),
                    None => None,
                };
                Ok((Ramp(a, b, c), DimensionVec::scalar()))
            }
            SafeDiv(a, b, c) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr2::infer_dimensions(*b, ctx)?);
                let c = match c {
                    Some(c_expr) => Some(Box::new(Expr2::infer_dimensions(*c_expr, ctx)?)),
                    None => None,
                };
                // Division result has broadcast shape of a and b
                let dims = match a.dims().broadcast_shape(b.dims()) {
                    Ok(dims) => dims,
                    Err(_) => return eqn_err!(MismatchedDimensions, 0, 0),
                };
                Ok((SafeDiv(a, b, c), dims))
            }
            Step(a, b) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr2::infer_dimensions(*b, ctx)?);
                Ok((Step(a, b), DimensionVec::scalar()))
            }
            Rank(a, bc_opt) => {
                let a = Box::new(Expr2::infer_dimensions(*a, ctx)?);
                let bc = match bc_opt {
                    Some((b, c_opt)) => {
                        let b = Box::new(Expr2::infer_dimensions(*b, ctx)?);
                        let c = match c_opt {
                            Some(c_expr) => Some(Box::new(Expr2::infer_dimensions(*c_expr, ctx)?)),
                            None => None,
                        };
                        Some((b, c))
                    }
                    None => None,
                };
                Ok((Rank(a, bc), DimensionVec::scalar()))
            }
        }
    }

    /// Apply subscript operations to dimensions
    fn apply_subscript_to_dims(
        base_dims: &DimensionVec,
        indices: &[IndexExpr2],
        loc: Loc,
    ) -> EquationResult<DimensionVec> {
        if indices.len() > base_dims.ndim() {
            return eqn_err!(MismatchedDimensions, loc.start, loc.end);
        }

        // Convert index expressions to SliceSpecs
        let mut slice_specs = Vec::new();
        for idx in indices.iter() {
            let spec = match idx {
                IndexExpr2::Wildcard(_, _) => SliceSpec::Wildcard,
                IndexExpr2::StarRange(name, _, _) => SliceSpec::DimName(name.clone()),
                IndexExpr2::Range(_start, _end, _, _) => {
                    // For now, assume constant ranges
                    // In a real implementation, we'd need to evaluate these
                    SliceSpec::Range(0, 10) // Placeholder
                }
                IndexExpr2::Expr(_) => SliceSpec::Index(0), // Placeholder
            };
            slice_specs.push(spec);
        }

        // Add wildcards for remaining dimensions
        for _ in indices.len()..base_dims.ndim() {
            slice_specs.push(SliceSpec::Wildcard);
        }

        match base_dims.slice_with_spec(&slice_specs) {
            Ok(dims) => Ok(dims),
            Err(_) => eqn_err!(MismatchedDimensions, loc.start, loc.end),
        }
    }

    /// Infer dimensions for binary operations
    fn infer_binary_op_dims(
        op: BinaryOp,
        left_dims: &DimensionVec,
        right_dims: &DimensionVec,
        loc: Loc,
    ) -> EquationResult<DimensionVec> {
        use BinaryOp::*;

        match op {
            // Arithmetic operations use broadcasting
            Add | Sub | Mul | Div | Mod | Exp => match left_dims.broadcast_shape(right_dims) {
                Ok(dims) => Ok(dims),
                Err(_) => eqn_err!(MismatchedDimensions, loc.start, loc.end),
            },
            // Comparison operations also use broadcasting
            Lt | Lte | Gt | Gte | Eq | Neq => match left_dims.broadcast_shape(right_dims) {
                Ok(dims) => Ok(dims),
                Err(_) => eqn_err!(MismatchedDimensions, loc.start, loc.end),
            },
            // Logical operations require same dimensions
            And | Or => {
                if left_dims.is_broadcast_compatible(right_dims) {
                    match left_dims.broadcast_shape(right_dims) {
                        Ok(dims) => Ok(dims),
                        Err(_) => eqn_err!(MismatchedDimensions, loc.start, loc.end),
                    }
                } else {
                    eqn_err!(MismatchedDimensions, loc.start, loc.end)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_dimension(name: &str, size: u32) -> Dimension {
        Dimension::Indexed(name.to_string(), size)
    }

    fn create_named_dimension(name: &str, elements: Vec<&str>) -> Dimension {
        Dimension::Named(
            name.to_string(),
            elements.into_iter().map(String::from).collect(),
        )
    }

    #[test]
    fn test_scalar_dimensions() {
        let scalar = DimensionVec::scalar();
        assert!(scalar.is_scalar());
        assert_eq!(scalar.ndim(), 0);
        assert_eq!(scalar.size(), 1);
        assert_eq!(scalar.shape(), vec![]);
    }

    #[test]
    fn test_basic_dimension_operations() {
        let dims = vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 2), 0, 2),
        ];
        let dim_vec = DimensionVec::new(dims);

        assert!(!dim_vec.is_scalar());
        assert_eq!(dim_vec.ndim(), 2);
        assert_eq!(dim_vec.size(), 6);
        assert_eq!(dim_vec.shape(), vec![3, 2]);
        assert_eq!(dim_vec.names(), vec!["Location", "Product"]);
    }

    #[test]
    fn test_is_broadcast_compatible_scalars() {
        let scalar = DimensionVec::scalar();
        let array = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("A", 3),
            0,
            3,
        )]);

        // Scalars are always broadcast compatible
        assert!(scalar.is_broadcast_compatible(&array));
        assert!(array.is_broadcast_compatible(&scalar));
        assert!(scalar.is_broadcast_compatible(&scalar));
    }

    #[test]
    fn test_is_broadcast_compatible_matching_dims() {
        let dims1 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 2), 0, 2),
        ]);
        let dims2 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 2), 0, 2),
        ]);

        assert!(dims1.is_broadcast_compatible(&dims2));
        assert!(dims2.is_broadcast_compatible(&dims1));
    }

    #[test]
    fn test_is_broadcast_compatible_different_names() {
        let dims1 = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("Location", 2),
            0,
            2,
        )]);
        let dims2 = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("Product", 2),
            0,
            2,
        )]);

        // Different dimension names are not compatible, even with same size
        assert!(!dims1.is_broadcast_compatible(&dims2));
    }

    #[test]
    fn test_is_broadcast_compatible_singleton_dimensions() {
        let dims1 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 1), 0, 1),
        ]);
        let dims2 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 4), 0, 4),
        ]);

        // Singleton dimensions (size 1) can broadcast
        assert!(dims1.is_broadcast_compatible(&dims2));
        assert!(dims2.is_broadcast_compatible(&dims1));
    }

    #[test]
    fn test_is_broadcast_compatible_missing_leading_dims() {
        let dims1 = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("Product", 4),
            0,
            4,
        )]);
        let dims2 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 4), 0, 4),
        ]);

        // Missing leading dimensions are ok
        assert!(dims1.is_broadcast_compatible(&dims2));
        assert!(dims2.is_broadcast_compatible(&dims1));
    }

    #[test]
    fn test_broadcast_shape_scalars() {
        let scalar = DimensionVec::scalar();
        let array = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("A", 3),
            0,
            3,
        )]);

        // Broadcasting with scalar returns the array shape
        assert_eq!(scalar.broadcast_shape(&array).unwrap(), array);
        assert_eq!(array.broadcast_shape(&scalar).unwrap(), array);
    }

    #[test]
    fn test_broadcast_shape_matching() {
        let dims1 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 2), 0, 2),
        ]);
        let dims2 = dims1.clone();

        assert_eq!(dims1.broadcast_shape(&dims2).unwrap(), dims1);
    }

    #[test]
    fn test_broadcast_shape_singleton() {
        let dims1 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 1), 0, 1),
        ]);
        let dims2 = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 4), 0, 4),
        ]);

        let result = dims1.broadcast_shape(&dims2).unwrap();
        assert_eq!(result.shape(), vec![3, 4]);
    }

    #[test]
    fn test_broadcast_shape_error() {
        let dims1 = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("Location", 3),
            0,
            3,
        )]);
        let dims2 = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("Location", 4),
            0,
            4,
        )]);

        // Incompatible dimensions
        assert!(dims1.broadcast_shape(&dims2).is_err());
    }

    #[test]
    fn test_is_assignable_to() {
        let scalar = DimensionVec::scalar();
        let array = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("A", 3),
            0,
            3,
        )]);

        // Scalar can be assigned to array
        assert!(scalar.is_assignable_to(&array));
        // Array cannot be assigned to scalar
        assert!(!array.is_assignable_to(&scalar));
        // Same dimensions are assignable
        assert!(array.is_assignable_to(&array));
    }

    #[test]
    fn test_slice_with_spec_wildcard() {
        let dims = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 2), 0, 2),
        ]);

        let specs = vec![SliceSpec::Wildcard, SliceSpec::Wildcard];
        let result = dims.slice_with_spec(&specs).unwrap();
        assert_eq!(result, dims);
    }

    #[test]
    fn test_slice_with_spec_index() {
        let dims = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 2), 0, 2),
        ]);

        // Selecting specific indices removes those dimensions
        let specs = vec![SliceSpec::Index(1), SliceSpec::Wildcard];
        let result = dims.slice_with_spec(&specs).unwrap();
        assert_eq!(result.ndim(), 1);
        assert_eq!(result.names(), vec!["Product"]);
    }

    #[test]
    fn test_slice_with_spec_range() {
        let dims = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 5), 0, 5),
            DimensionRange::new(create_test_dimension("Product", 3), 0, 3),
        ]);

        let specs = vec![SliceSpec::Range(1, 4), SliceSpec::Wildcard];
        let result = dims.slice_with_spec(&specs).unwrap();
        assert_eq!(result.ndim(), 2);
        assert_eq!(result.shape(), vec![3, 3]); // Range 1:4 has length 3
    }

    #[test]
    fn test_slice_with_spec_dimname() {
        let dims = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Product", 2), 0, 2),
        ]);

        // Matching dimension names
        let specs = vec![
            SliceSpec::DimName("Location".to_string()),
            SliceSpec::DimName("Product".to_string()),
        ];
        let result = dims.slice_with_spec(&specs).unwrap();
        assert_eq!(result, dims);

        // Non-matching dimension name
        let bad_specs = vec![
            SliceSpec::DimName("Location".to_string()),
            SliceSpec::DimName("Time".to_string()),
        ];
        assert!(dims.slice_with_spec(&bad_specs).is_err());
    }

    #[test]
    fn test_slice_with_spec_mixed() {
        let dims = DimensionVec::new(vec![
            DimensionRange::new(create_test_dimension("Location", 5), 0, 5),
            DimensionRange::new(create_test_dimension("Product", 3), 0, 3),
            DimensionRange::new(create_test_dimension("Time", 10), 0, 10),
        ]);

        let specs = vec![
            SliceSpec::Index(2),    // Remove Location dimension
            SliceSpec::Wildcard,    // Keep Product dimension
            SliceSpec::Range(0, 5), // Keep Time dimension but truncate
        ];
        let result = dims.slice_with_spec(&specs).unwrap();
        assert_eq!(result.ndim(), 2);
        assert_eq!(result.names(), vec!["Product", "Time"]);
        assert_eq!(result.shape(), vec![3, 5]);
    }

    #[test]
    fn test_slice_with_spec_error_wrong_length() {
        let dims = DimensionVec::new(vec![DimensionRange::new(
            create_test_dimension("Location", 3),
            0,
            3,
        )]);

        let specs = vec![SliceSpec::Wildcard, SliceSpec::Wildcard]; // Too many specs
        assert!(dims.slice_with_spec(&specs).is_err());
    }

    #[test]
    fn test_named_dimensions() {
        let dims = DimensionVec::new(vec![
            DimensionRange::new(
                create_named_dimension("Location", vec!["Boston", "Chicago", "LA"]),
                0,
                3,
            ),
            DimensionRange::new(
                create_named_dimension("Product", vec!["Shirts", "Pants"]),
                0,
                2,
            ),
        ]);

        assert_eq!(dims.ndim(), 2);
        assert_eq!(dims.size(), 6);
        assert_eq!(dims.names(), vec!["Location", "Product"]);
    }

    #[test]
    fn test_transpose_basic() {
        // Test basic transpose functionality (reverses dimensions)
        let dims = vec![
            DimensionRange::new(create_test_dimension("DimA", 3), 0, 3),
            DimensionRange::new(create_test_dimension("DimB", 4), 0, 4),
        ];
        let dim_vec = DimensionVec::new(dims);

        let transposed = dim_vec.transpose();

        // Should reverse the dimension order
        assert_eq!(transposed.ndim(), 2);
        assert_eq!(transposed.names(), vec!["DimB", "DimA"]);
        assert_eq!(transposed.shape(), vec![4, 3]);
    }

    #[test]
    fn test_transpose_scalar() {
        // Test transpose of scalar (should remain scalar)
        let scalar_dims = DimensionVec::scalar();
        let transposed = scalar_dims.transpose();

        assert!(transposed.is_scalar());
        assert_eq!(transposed.ndim(), 0);
        assert_eq!(transposed.size(), 1);
    }

    #[test]
    fn test_transpose_1d() {
        // Test transpose of 1D array (should reverse to same array)
        let dims = vec![DimensionRange::new(create_test_dimension("DimA", 5), 0, 5)];
        let dim_vec = DimensionVec::new(dims);

        let transposed = dim_vec.transpose();

        assert_eq!(transposed.ndim(), 1);
        assert_eq!(transposed.names(), vec!["DimA"]);
        assert_eq!(transposed.shape(), vec![5]);
    }

    #[test]
    fn test_transpose_3d() {
        // Test transpose of 3D array
        let dims = vec![
            DimensionRange::new(create_test_dimension("DimA", 2), 0, 2),
            DimensionRange::new(create_test_dimension("DimB", 3), 0, 3),
            DimensionRange::new(create_test_dimension("DimC", 4), 0, 4),
        ];
        let dim_vec = DimensionVec::new(dims);

        let transposed = dim_vec.transpose();

        // Should reverse all dimensions
        assert_eq!(transposed.ndim(), 3);
        assert_eq!(transposed.names(), vec!["DimC", "DimB", "DimA"]);
        assert_eq!(transposed.shape(), vec![4, 3, 2]);
    }

    #[test]
    fn test_transpose_double() {
        // Test double transpose returns to original
        let dims = vec![
            DimensionRange::new(create_test_dimension("DimA", 3), 0, 3),
            DimensionRange::new(create_test_dimension("DimB", 4), 0, 4),
        ];
        let original = DimensionVec::new(dims);

        let double_transposed = original.transpose().transpose();

        assert_eq!(original, double_transposed);
    }
}
