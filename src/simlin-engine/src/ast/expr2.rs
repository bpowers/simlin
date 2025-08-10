// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{EquationResult, Ident};
use crate::dimensions::Dimension;
use crate::eqn_err;

/// Simplified array bounds tracking for type checking phase
///
/// During the type checking phase (Expr2), we only need to track:
/// - Whether this is a named variable or a temporary
/// - The maximum size of each dimension
///
/// All complex view calculations (strides, offsets, etc.) are deferred
/// to the compiler phase where we have more context.
#[derive(PartialEq, Clone, Debug)]
pub enum ArrayBounds {
    /// Array bounds for a named variable (from the model)
    Named {
        /// Variable name
        name: String,
        /// Maximum size of each dimension
        dims: Vec<usize>,
        /// Dimension names (if available)
        dim_names: Option<Vec<String>>,
    },
    /// Array bounds for a temporary (intermediate result)
    Temp {
        /// Temporary ID allocated for this array expression
        id: u32,
        /// Maximum size of each dimension
        dims: Vec<usize>,
        /// Dimension names (if available)
        dim_names: Option<Vec<String>>,
    },
}

impl ArrayBounds {
    /// Returns the total number of elements in the array
    pub fn size(&self) -> usize {
        match self {
            ArrayBounds::Named { dims, .. } | ArrayBounds::Temp { dims, .. } => {
                dims.iter().product()
            }
        }
    }

    /// Returns the dimensions of the array
    pub fn dims(&self) -> &[usize] {
        match self {
            ArrayBounds::Named { dims, .. } | ArrayBounds::Temp { dims, .. } => dims,
        }
    }

    /// Returns the dimension names (if available)
    pub fn dim_names(&self) -> Option<&[String]> {
        match self {
            ArrayBounds::Named { dim_names, .. } | ArrayBounds::Temp { dim_names, .. } => {
                dim_names.as_deref()
            }
        }
    }
}

/// IndexExpr represents a parsed equation, after calls to
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
    pub(crate) fn from<C: Expr2Context>(expr: IndexExpr1, ctx: &mut C) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr1::Wildcard(loc) => IndexExpr2::Wildcard(loc),
            IndexExpr1::StarRange(ident, loc) => IndexExpr2::StarRange(ident.to_ident(), loc),
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
    Var(Ident, Option<ArrayBounds>, Loc),
    App(BuiltinFn<Expr2>, Option<ArrayBounds>, Loc),
    Subscript(Ident, Vec<IndexExpr2>, Option<ArrayBounds>, Loc),
    Op1(UnaryOp, Box<Expr2>, Option<ArrayBounds>, Loc),
    Op2(BinaryOp, Box<Expr2>, Box<Expr2>, Option<ArrayBounds>, Loc),
    If(Box<Expr2>, Box<Expr2>, Box<Expr2>, Option<ArrayBounds>, Loc),
}

/// Context trait for converting Expr1 to Expr2
/// Provides access to variable dimension information and temp ID allocation
pub trait Expr2Context {
    /// Get the dimensions of a variable, or None if it's a scalar
    fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>>;

    /// Allocate a new temp ID for the current equation
    fn allocate_temp_id(&mut self) -> u32;

    /// Check if an identifier is a dimension name
    fn is_dimension_name(&self, ident: &str) -> bool;

    /// Check if we're in an array context (processing an arrayed or apply-to-all equation)
    fn is_array_context(&self) -> bool;
}

impl Expr2 {
    /// Extract the array bounds from an expression, if it has one
    fn get_array_bounds(&self) -> Option<&ArrayBounds> {
        match self {
            Expr2::Const(_, _, _) => None,
            Expr2::Var(_, array_bounds, _) => array_bounds.as_ref(),
            Expr2::App(_, array_bounds, _) => array_bounds.as_ref(),
            Expr2::Subscript(_, _, array_bounds, _) => array_bounds.as_ref(),
            Expr2::Op1(_, _, array_bounds, _) => array_bounds.as_ref(),
            Expr2::Op2(_, _, _, array_bounds, _) => array_bounds.as_ref(),
            Expr2::If(_, _, _, array_bounds, _) => array_bounds.as_ref(),
        }
    }

    /// Allocates a new temp ID for an array with given dimensions
    fn allocate_temp_array<C: Expr2Context>(ctx: &mut C, dims: Vec<usize>) -> ArrayBounds {
        ArrayBounds::Temp {
            id: ctx.allocate_temp_id(),
            dims,
            dim_names: None, // Temp arrays don't have dimension names initially
        }
    }

    /// Allocates a new temp ID for an array with given dimensions and names
    fn allocate_temp_array_with_names<C: Expr2Context>(
        ctx: &mut C,
        dims: Vec<usize>,
        names: Vec<String>,
    ) -> ArrayBounds {
        ArrayBounds::Temp {
            id: ctx.allocate_temp_id(),
            dims,
            dim_names: Some(names),
        }
    }

    fn unify_array_bounds<C: Expr2Context>(
        ctx: &mut C,
        l: Option<&ArrayBounds>,
        r: Option<&ArrayBounds>,
        loc: Loc,
    ) -> EquationResult<Option<ArrayBounds>> {
        match (l, r) {
            // Both sides are arrays - check dimensions match
            (Some(left), Some(right)) => {
                // Check if dimensions can be unified (with possible reordering)
                let (dims, dim_names) = Self::unify_dims_with_names(
                    left.dims(),
                    left.dim_names(),
                    right.dims(),
                    right.dim_names(),
                    loc,
                )?;

                if let Some(names) = dim_names {
                    Ok(Some(Self::allocate_temp_array_with_names(ctx, dims, names)))
                } else {
                    Ok(Some(Self::allocate_temp_array(ctx, dims)))
                }
            }
            // one side is array, the other is scalar: broadcast
            (Some(array), None) | (None, Some(array)) => {
                if let Some(names) = array.dim_names() {
                    Ok(Some(Self::allocate_temp_array_with_names(
                        ctx,
                        array.dims().to_vec(),
                        names.to_vec(),
                    )))
                } else {
                    Ok(Some(Self::allocate_temp_array(ctx, array.dims().to_vec())))
                }
            }
            // Both scalars
            (None, None) => Ok(None),
        }
    }

    /// Check if two array dimension lists are compatible for element-wise operations
    fn unify_dims(a: &[usize], b: &[usize], loc: Loc) -> EquationResult<Vec<usize>> {
        if a.len() != b.len() {
            return eqn_err!(MismatchedDimensions, loc.start, loc.end);
        }

        let dims: EquationResult<Vec<usize>> = a
            .iter()
            .zip(b.iter())
            .map(|(d1, d2)| {
                if d1 == d2 {
                    Ok(*d1)
                } else {
                    eqn_err!(MismatchedDimensions, loc.start, loc.end)
                }
            })
            .collect();

        dims
    }

    /// Check if two array dimension lists are compatible for element-wise operations
    /// This version also handles dimension names and allows reordering
    fn unify_dims_with_names(
        a_dims: &[usize],
        a_names: Option<&[String]>,
        b_dims: &[usize],
        b_names: Option<&[String]>,
        loc: Loc,
    ) -> EquationResult<(Vec<usize>, Option<Vec<String>>)> {
        // If we don't have names for both, fall back to position-based matching
        let (a_names, b_names) = match (a_names, b_names) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                // Fall back to old behavior
                let dims = Self::unify_dims(a_dims, b_dims, loc)?;
                return Ok((dims, None));
            }
        };

        // Check if dimensions have same count
        if a_dims.len() != b_dims.len() {
            return eqn_err!(MismatchedDimensions, loc.start, loc.end);
        }

        // Try to match dimensions by name
        use crate::compiler::find_dimension_reordering;

        // Check if b can be reordered to match a's dimension order
        if let Some(reordering) = find_dimension_reordering(b_names, a_names) {
            // Build the unified dimensions using a's order
            let mut unified_dims = Vec::with_capacity(a_dims.len());
            let mut has_mismatch = false;

            for (i, &a_dim) in a_dims.iter().enumerate() {
                let b_idx = reordering[i];
                let b_dim = b_dims[b_idx];

                if a_dim != b_dim {
                    has_mismatch = true;
                    break;
                }
                unified_dims.push(a_dim);
            }

            if has_mismatch {
                return eqn_err!(MismatchedDimensions, loc.start, loc.end);
            }

            return Ok((unified_dims, Some(a_names.to_vec())));
        }

        // If reordering doesn't work, dimensions are incompatible
        eqn_err!(MismatchedDimensions, loc.start, loc.end)
    }

    pub(crate) fn from<C: Expr2Context>(expr: Expr1, ctx: &mut C) -> EquationResult<Self> {
        let expr = match expr {
            Expr1::Const(s, n, loc) => Expr2::Const(s, n, loc),
            Expr1::Var(id, loc) => {
                // Check if this is a dimension name being used in a scalar context
                // In array contexts, dimension names are allowed and will be converted to indices
                if ctx.is_dimension_name(id.as_str()) && !ctx.is_array_context() {
                    return eqn_err!(DimensionInScalarContext, loc.start, loc.end);
                }

                let array_bounds = if let Some(dims) = ctx.get_dimensions(id.as_str()) {
                    let dim_sizes: Vec<usize> = dims.iter().map(|d| d.len()).collect();
                    let dim_names: Vec<String> =
                        dims.iter().map(|d| d.name().to_string()).collect();
                    Some(ArrayBounds::Named {
                        name: id.to_ident(),
                        dims: dim_sizes,
                        dim_names: Some(dim_names),
                    })
                } else {
                    None
                };
                Expr2::Var(id.to_ident(), array_bounds, loc)
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
                let array_bounds = if let Some(dims) = ctx.get_dimensions(id.as_str()) {
                    // For now, compute maximum bounds after subscripting
                    // In the simplified design, we just track the result dimensions
                    // The actual subscript logic will be handled in the compiler

                    let mut result_dims = Vec::new();

                    // Simple dimension calculation - count wildcards to determine result dims
                    for (i, arg) in args.iter().enumerate() {
                        if i < dims.len() {
                            match arg {
                                IndexExpr2::Wildcard(_) => {
                                    result_dims.push(dims[i].len());
                                }
                                IndexExpr2::Range(_start, _end, _) => {
                                    // For ranges, we'd need to evaluate start/end
                                    // For now, use the full dimension size as max bound
                                    result_dims.push(dims[i].len());
                                }
                                IndexExpr2::StarRange(_, _) => {
                                    // Star ranges keep the dimension
                                    result_dims.push(dims[i].len());
                                }
                                IndexExpr2::Expr(_) | IndexExpr2::DimPosition(_, _) => {
                                    // These reduce the dimension
                                }
                            }
                        }
                    }

                    if result_dims.is_empty() {
                        None // Result is scalar
                    } else {
                        Some(Self::allocate_temp_array(ctx, result_dims))
                    }
                } else {
                    None // Scalar variable or unknown variable
                };

                Expr2::Subscript(id.to_ident(), args, array_bounds, loc)
            }
            Expr1::Op1(op, l, loc) => {
                let l_expr = Expr2::from(*l, ctx)?;

                // Compute array bounds for unary operations
                let array_bounds = match (&op, l_expr.get_array_bounds()) {
                    (UnaryOp::Transpose, Some(bounds)) => {
                        // Transpose reverses dimensions
                        let mut transposed_dims = bounds.dims().to_vec();
                        transposed_dims.reverse();
                        Some(Self::allocate_temp_array(ctx, transposed_dims))
                    }
                    (_, Some(bounds)) => {
                        // Other unary ops preserve array structure
                        Some(Self::allocate_temp_array(ctx, bounds.dims().to_vec()))
                    }
                    _ => None,
                };

                Expr2::Op1(op, Box::new(l_expr), array_bounds, loc)
            }
            Expr1::Op2(op, l, r, loc) => {
                let l_expr = Expr2::from(*l, ctx)?;
                let r_expr = Expr2::from(*r, ctx)?;

                // Compute array bounds for binary operations
                let array_bounds = Self::unify_array_bounds(
                    ctx,
                    l_expr.get_array_bounds(),
                    r_expr.get_array_bounds(),
                    loc,
                )?;

                Expr2::Op2(op, Box::new(l_expr), Box::new(r_expr), array_bounds, loc)
            }
            Expr1::If(cond, t, f, loc) => {
                let cond_expr = Expr2::from(*cond, ctx)?;
                let t_expr = Expr2::from(*t, ctx)?;
                let f_expr = Expr2::from(*f, ctx)?;

                // Compute array bounds for if expressions
                let array_bounds = Self::unify_array_bounds(
                    ctx,
                    t_expr.get_array_bounds(),
                    f_expr.get_array_bounds(),
                    loc,
                )?;

                Expr2::If(
                    Box::new(cond_expr),
                    Box::new(t_expr),
                    Box::new(f_expr),
                    array_bounds,
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
#[cfg(test)]
fn const_int_eval(ast: &Expr2) -> EquationResult<i32> {
    use float_cmp::approx_eq;
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
    use std::collections::HashMap;
    use std::iter::Iterator;

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

        fn is_dimension_name(&self, _ident: &str) -> bool {
            // For tests, we don't have dimension names
            false
        }

        fn is_array_context(&self) -> bool {
            // For tests, assume we're not in array context unless specifically testing that
            false
        }
    }

    #[test]
    fn test_array_bounds() {
        // Test Named variant
        let named_bounds = ArrayBounds::Named {
            name: "array_var".to_string(),
            dims: vec![3, 4],
            dim_names: None,
        };
        match &named_bounds {
            ArrayBounds::Named { name, dims, .. } => {
                assert_eq!(name, "array_var");
                assert_eq!(dims, &vec![3, 4]);
            }
            _ => panic!("Expected Named variant"),
        }
        assert_eq!(named_bounds.dims(), &[3, 4]);
        assert_eq!(named_bounds.size(), 12); // 3 * 4 = 12

        // Test Temp variant
        let temp_bounds = ArrayBounds::Temp {
            id: 5,
            dims: vec![2, 3],
            dim_names: None,
        };
        match &temp_bounds {
            ArrayBounds::Temp { id, dims, .. } => {
                assert_eq!(*id, 5);
                assert_eq!(dims, &vec![2, 3]);
            }
            _ => panic!("Expected Temp variant"),
        }
        assert_eq!(temp_bounds.dims(), &[2, 3]);
        assert_eq!(temp_bounds.size(), 6); // 2 * 3 = 6

        // Test scalar (empty dims)
        let scalar_bounds = ArrayBounds::Temp {
            id: 1,
            dims: vec![],
            dim_names: None,
        };
        assert_eq!(scalar_bounds.size(), 1); // Empty product = 1

        // Test 1D array
        let bounds_1d = ArrayBounds::Named {
            name: "vector".to_string(),
            dims: vec![5],
            dim_names: None,
        };
        assert_eq!(bounds_1d.size(), 5);

        // Test 3D array
        let bounds_3d = ArrayBounds::Temp {
            id: 3,
            dims: vec![2, 3, 4],
            dim_names: None,
        };
        assert_eq!(bounds_3d.size(), 24); // 2 * 3 * 4 = 24
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
    fn test_expr2_from_scalar_var() {
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Test scalar variable (no dimensions)
        let var_expr = Expr1::Var(CanonicalIdent::from_raw("scalar_var"), Loc::default());
        let expr2 = Expr2::from(var_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Var(id, array_bounds, _) => {
                assert_eq!(id, "scalar_var");
                assert!(array_bounds.is_none()); // Scalar has no array bounds
            }
            _ => panic!("Expected Var expression"),
        }
    }

    #[test]
    fn test_expr2_from_array_var() {
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[3, 4]));

        // Test array variable with dimensions
        let var_expr = Expr1::Var(CanonicalIdent::from_raw("array_var"), Loc::default());
        let expr2 = Expr2::from(var_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Var(id, array_bounds, _) => {
                assert_eq!(id, "array_var");
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match bounds {
                    ArrayBounds::Named { name, dims, .. } => {
                        assert_eq!(name, "array_var");
                        assert_eq!(dims, vec![3, 4]);
                    }
                    _ => panic!("Expected Named array bounds"),
                }
            }
            _ => panic!("Expected Var expression"),
        }
    }

    #[test]
    fn test_expr2_subscript_reduces_dimensions() {
        use crate::ast::expr1::{Expr1, IndexExpr1};
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("matrix".to_string(), indexed_dims(&[3, 4]));

        // Test subscript with one index reduces dimension
        let subscript_expr = Expr1::Subscript(
            CanonicalIdent::from_raw("matrix"),
            vec![
                IndexExpr1::Expr(Expr1::Const("1".to_string(), 1.0, Loc::default())),
                IndexExpr1::Wildcard(Loc::default()),
            ],
            Loc::default(),
        );
        let expr2 = Expr2::from(subscript_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Subscript(id, args, array_bounds, _) => {
                assert_eq!(id, "matrix");
                assert_eq!(args.len(), 2);
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match bounds {
                    ArrayBounds::Temp { id, dims, .. } => {
                        assert_eq!(id, 0); // First temp allocation
                        assert_eq!(dims, vec![4]); // Only second dimension remains
                    }
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Subscript expression"),
        }
    }

    #[test]
    fn test_expr2_subscript_scalar_result() {
        use crate::ast::expr1::{Expr1, IndexExpr1};
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("vector".to_string(), indexed_dims(&[5]));

        // Test subscript that results in scalar
        let subscript_expr = Expr1::Subscript(
            CanonicalIdent::from_raw("vector"),
            vec![IndexExpr1::Expr(Expr1::Const(
                "2".to_string(),
                2.0,
                Loc::default(),
            ))],
            Loc::default(),
        );
        let expr2 = Expr2::from(subscript_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Subscript(id, args, array_bounds, _) => {
                assert_eq!(id, "vector");
                assert_eq!(args.len(), 1);
                assert!(array_bounds.is_none()); // Scalar result
            }
            _ => panic!("Expected Subscript expression"),
        }
    }

    #[test]
    fn test_expr2_unary_op_preserves_array() {
        use crate::ast::UnaryOp;
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 3]));

        // Test unary negative preserves array dimensions
        let neg_expr = Expr1::Op1(
            UnaryOp::Negative,
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array_var"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(neg_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op1(UnaryOp::Negative, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match bounds {
                    ArrayBounds::Temp { id, dims, .. } => {
                        assert_eq!(id, 0); // First temp allocation
                        assert_eq!(dims, vec![2, 3]); // Dimensions preserved
                    }
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Op1 expression"),
        }
    }

    #[test]
    fn test_expr2_transpose_reverses_dims() {
        use crate::ast::UnaryOp;
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("matrix".to_string(), indexed_dims(&[3, 4]));

        // Test transpose reverses dimensions
        let transpose_expr = Expr1::Op1(
            UnaryOp::Transpose,
            Box::new(Expr1::Var(CanonicalIdent::from_raw("matrix"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(transpose_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op1(UnaryOp::Transpose, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match bounds {
                    ArrayBounds::Temp { id, dims, .. } => {
                        assert_eq!(id, 0); // First temp allocation
                        assert_eq!(dims, vec![4, 3]); // Dimensions reversed
                    }
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Op1 expression"),
        }
    }

    #[test]
    fn test_expr2_binary_op_array_scalar() {
        use crate::ast::BinaryOp;
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 3]));

        // Test array + scalar (broadcasting)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array_var"), Loc::default())),
            Box::new(Expr1::Const("10".to_string(), 10.0, Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(add_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op2(BinaryOp::Add, _, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match bounds {
                    ArrayBounds::Temp { id, dims, .. } => {
                        assert_eq!(id, 0); // First temp allocation
                        assert_eq!(dims, vec![2, 3]); // Array dimensions preserved
                    }
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Op2 expression"),
        }
    }

    #[test]
    fn test_expr2_binary_op_matching_arrays() {
        use crate::ast::BinaryOp;
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array1".to_string(), indexed_dims(&[3, 4]));
        ctx.dimensions
            .insert("array2".to_string(), indexed_dims(&[3, 4]));

        // Test array + array (matching dimensions)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array1"), Loc::default())),
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array2"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(add_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op2(BinaryOp::Add, _, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match bounds {
                    ArrayBounds::Temp { id, dims, .. } => {
                        assert_eq!(id, 0); // First temp allocation
                        assert_eq!(dims, vec![3, 4]); // Dimensions preserved
                    }
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Op2 expression"),
        }
    }

    #[test]
    fn test_expr2_if_array_branches() {
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 2]));

        // Test if expression with array in both branches
        let if_expr = Expr1::If(
            Box::new(Expr1::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array_var"), Loc::default())),
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array_var"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(if_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::If(_, _, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match bounds {
                    ArrayBounds::Temp { id, dims, .. } => {
                        assert_eq!(id, 0); // First temp allocation
                        assert_eq!(dims, vec![2, 2]); // Dimensions preserved
                    }
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected If expression"),
        }
    }

    #[test]
    fn test_expr2_temp_id_allocation() {
        use crate::ast::{BinaryOp, UnaryOp};
        use crate::ast::expr1::Expr1;
        use crate::common::CanonicalIdent;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array1".to_string(), indexed_dims(&[2, 2]));
        ctx.dimensions
            .insert("array2".to_string(), indexed_dims(&[2, 2]));

        // Create multiple array operations to test temp ID allocation
        // First operation: -array1 (should get temp_id 0)
        let neg_expr = Expr1::Op1(
            UnaryOp::Negative,
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array1"), Loc::default())),
            Loc::default(),
        );
        let expr2_1 = Expr2::from(neg_expr, &mut ctx).unwrap();

        // Second operation: array1 + array2 (should get temp_id 1)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array1"), Loc::default())),
            Box::new(Expr1::Var(CanonicalIdent::from_raw("array2"), Loc::default())),
            Loc::default(),
        );
        let expr2_2 = Expr2::from(add_expr, &mut ctx).unwrap();

        // Check first operation got temp_id 0
        match expr2_1 {
            Expr2::Op1(_, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                match array_bounds.unwrap() {
                    ArrayBounds::Temp { id, .. } => assert_eq!(id, 0),
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Op1"),
        }

        // Check second operation got temp_id 1
        match expr2_2 {
            Expr2::Op2(_, _, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                match array_bounds.unwrap() {
                    ArrayBounds::Temp { id, .. } => assert_eq!(id, 1),
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Op2"),
        }
    }
}
