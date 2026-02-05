// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{Canonical, CanonicalDimensionName, EquationResult, Ident};
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub enum IndexExpr2 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(CanonicalDimensionName, Loc),
    Range(Expr2, Expr2, Loc),
    DimPosition(u32, Loc),
    Expr(Expr2),
}

impl IndexExpr2 {
    pub(crate) fn from<C: Expr2Context>(expr: IndexExpr1, ctx: &mut C) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr1::Wildcard(loc) => IndexExpr2::Wildcard(loc),
            IndexExpr1::StarRange(ident, loc) => {
                IndexExpr2::StarRange(CanonicalDimensionName::from_raw(ident.as_str()), loc)
            }
            IndexExpr1::Range(l, r, loc) => {
                IndexExpr2::Range(Expr2::from(l, ctx)?, Expr2::from(r, ctx)?, loc)
            }
            IndexExpr1::DimPosition(n, loc) => IndexExpr2::DimPosition(n, loc),
            IndexExpr1::Expr(e) => IndexExpr2::Expr(Expr2::from(e, ctx)?),
        };

        Ok(expr)
    }

    /// Get the source location of this index expression.
    pub fn get_loc(&self) -> Loc {
        match self {
            IndexExpr2::Wildcard(loc) => *loc,
            IndexExpr2::StarRange(_, loc) => *loc,
            IndexExpr2::Range(_, _, loc) => *loc,
            IndexExpr2::DimPosition(_, loc) => *loc,
            IndexExpr2::Expr(e) => e.get_loc(),
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            IndexExpr2::Wildcard(_) => None,
            IndexExpr2::StarRange(v, loc) => {
                if v.as_str() == ident {
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub enum Expr2 {
    Const(String, f64, Loc),
    Var(Ident<Canonical>, Option<ArrayBounds>, Loc),
    App(BuiltinFn<Expr2>, Option<ArrayBounds>, Loc),
    Subscript(Ident<Canonical>, Vec<IndexExpr2>, Option<ArrayBounds>, Loc),
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

    /// Get the length of a dimension by its canonical name.
    /// Used for StarRange subscripts to determine result dimensions.
    fn get_dimension_len(&self, name: &CanonicalDimensionName) -> Option<usize>;

    /// Check if a dimension is an indexed dimension (vs. named dimension).
    /// Indexed dimensions can be matched by size with different names.
    fn is_indexed_dimension(&self, name: &str) -> bool;

    /// Check if dimension union is allowed for named dimensions.
    /// When true, arrays with different named dimensions can be combined
    /// in expressions, producing a cross-product of their dimensions.
    /// This is used inside array reduction builtins (SUM, MEAN, etc.)
    /// where cross-dimension expressions are semantically valid.
    fn allow_dimension_union(&self) -> bool {
        false
    }

    /// Set whether dimension union is allowed.
    /// Returns the previous value so it can be restored.
    fn set_allow_dimension_union(&mut self, _allow: bool) -> bool {
        // Default implementation for contexts that don't support this
        false
    }
}

impl Expr2 {
    /// Extract the array bounds from an expression, if it has one
    pub(crate) fn get_array_bounds(&self) -> Option<&ArrayBounds> {
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
                    ctx,
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

    /// Check if two array dimension lists are compatible for element-wise operations.
    /// This version handles dimension names and supports both reordering and broadcasting.
    ///
    /// Broadcasting rules:
    /// - If both arrays have the same dimensions (possibly reordered), sizes must match
    /// - If one array's dimensions are a SUBSET of the other, broadcast the smaller one
    /// - If dimensions are disjoint (neither is subset of other), return error
    /// - SPECIAL: Indexed dimensions with same size are considered compatible even with different names
    /// - Output dimension order: larger array's dimensions (or first if equal)
    fn unify_dims_with_names<C: Expr2Context>(
        ctx: &C,
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

        // Check subset relationships with indexed dimension size matching.
        // Use two-pass matching with usage tracking to avoid the bug where
        // multiple source dimensions could all "match" the same target dimension.
        //
        // For example, a[X(3), Y(3)] vs b[Z(3)]:
        // - Without usage tracking: X matches Z, Y matches Z → incorrectly reports a can match b
        // - With usage tracking: X matches Z, Y has no remaining match → correctly reports failure

        /// Check if all source dimensions can be matched to UNIQUE target dimensions.
        /// Uses two-pass matching: name matches first, then size matches.
        fn can_all_match<C: Expr2Context>(
            ctx: &C,
            source_names: &[String],
            source_dims: &[usize],
            target_names: &[String],
            target_dims: &[usize],
        ) -> bool {
            let mut target_used = vec![false; target_names.len()];

            // PASS 1: Assign name matches first (reserve them)
            let mut source_matched = vec![false; source_names.len()];
            for (source_idx, (name, &_size)) in
                source_names.iter().zip(source_dims.iter()).enumerate()
            {
                for (target_idx, target_name) in target_names.iter().enumerate() {
                    if !target_used[target_idx] && target_name == name {
                        target_used[target_idx] = true;
                        source_matched[source_idx] = true;
                        break;
                    }
                }
            }

            // PASS 2: For remaining sources, try size-based matching (indexed dims only)
            for (source_idx, (name, &size)) in
                source_names.iter().zip(source_dims.iter()).enumerate()
            {
                if source_matched[source_idx] {
                    continue; // Already matched by name
                }

                if !ctx.is_indexed_dimension(name) {
                    return false; // Named dim without name match fails
                }

                let mut found = false;
                for (target_idx, (target_name, &target_size)) in
                    target_names.iter().zip(target_dims.iter()).enumerate()
                {
                    if !target_used[target_idx]
                        && ctx.is_indexed_dimension(target_name)
                        && size == target_size
                    {
                        target_used[target_idx] = true;
                        found = true;
                        break;
                    }
                }

                if !found {
                    return false;
                }
            }

            true
        }

        // Check if all dimensions in a can be matched to UNIQUE dimensions in b
        let a_can_match_b = can_all_match(ctx, a_names, a_dims, b_names, b_dims);

        // Check if all dimensions in b can be matched to UNIQUE dimensions in a
        let b_can_match_a = can_all_match(ctx, b_names, b_dims, a_names, a_dims);

        // Handle UNION case: neither is a subset of the other
        // This happens when we're broadcasting, e.g., a[X] + b[Y] → result[X,Y]
        // IMPORTANT: UNION broadcasting is only allowed for INDEXED dimensions,
        // UNLESS we're inside an array reduction builtin (SUM, MEAN, etc.) where
        // dimension union is explicitly requested (allow_dimension_union=true).
        // Named dimensions with semantic meaning (like Cities, Products) should NOT
        // be combined just because they have the same size in normal A2A contexts.
        if !a_can_match_b && !b_can_match_a {
            // Check that all unmatched dimensions are indexed (not named)
            // If any unmatched dimension is named, return an error
            // EXCEPTION: When inside an array reduction builtin, allow named dimension unions
            let allow_union = ctx.allow_dimension_union();
            for (a_name, &a_size) in a_names.iter().zip(a_dims.iter()) {
                let has_match =
                    Self::find_matching_dimension(ctx, a_name, a_size, b_names, b_dims).is_some();
                if !has_match && !ctx.is_indexed_dimension(a_name) && !allow_union {
                    // This is a named dimension that doesn't match anything in b
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }
            }
            for (b_name, &b_size) in b_names.iter().zip(b_dims.iter()) {
                let has_match =
                    Self::find_matching_dimension(ctx, b_name, b_size, a_names, a_dims).is_some();
                if !has_match && !ctx.is_indexed_dimension(b_name) && !allow_union {
                    // This is a named dimension that doesn't match anything in a
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }
            }

            // All unmatched dimensions are indexed - safe to build a union
            // Start with all of a's dimensions, then add b's unmatched dimensions
            let mut unified_dims = Vec::new();
            let mut unified_names = Vec::new();

            // Add all of a's dimensions first
            for (name, &size) in a_names.iter().zip(a_dims.iter()) {
                unified_dims.push(size);
                unified_names.push(name.clone());
            }

            // Add b's dimensions that aren't already matched
            for (b_name, &b_size) in b_names.iter().zip(b_dims.iter()) {
                // Check if this b dimension matches any a dimension
                let match_result =
                    Self::find_matching_dimension(ctx, b_name, b_size, a_names, a_dims);

                match match_result {
                    Some((_matched_name, matched_size)) => {
                        // Dimension matched - verify sizes are equal
                        // (could differ if matched by name but defined with different sizes)
                        if b_size != matched_size {
                            return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                        }
                        // Already present in a, don't add again
                    }
                    None => {
                        // This is a new dimension from b - add it to the union
                        unified_dims.push(b_size);
                        unified_names.push(b_name.clone());
                    }
                }
            }

            return Ok((unified_dims, Some(unified_names)));
        }

        // One is a subset of the other - use the larger array as the output dimension order
        // If equal, use a's order
        let (primary_names, primary_dims, secondary_names, secondary_dims) =
            if b_can_match_a || a_dims.len() >= b_dims.len() {
                (a_names, a_dims, b_names, b_dims)
            } else {
                (b_names, b_dims, a_names, a_dims)
            };

        // Build output dimensions in the primary array's order
        let mut unified_dims = Vec::new();
        let mut unified_names = Vec::new();

        for (name, &size) in primary_names.iter().zip(primary_dims.iter()) {
            // Check if this dimension has a matching dimension in secondary
            // Match can be by name or by size (for indexed dims)
            let matching_secondary =
                Self::find_matching_dimension(ctx, name, size, secondary_names, secondary_dims);

            if let Some((_matched_name, matched_size)) = matching_secondary {
                // Found a match - sizes must be equal
                if size != matched_size {
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }
            }

            unified_dims.push(size);
            unified_names.push(name.clone());
        }

        Ok((unified_dims, Some(unified_names)))
    }

    /// Find a matching dimension in the secondary array.
    /// Returns the matched dimension's name and size if found.
    fn find_matching_dimension<'a, C: Expr2Context>(
        ctx: &C,
        name: &str,
        size: usize,
        secondary_names: &'a [String],
        secondary_dims: &[usize],
    ) -> Option<(&'a str, usize)> {
        // First try name match
        for (sec_name, &sec_size) in secondary_names.iter().zip(secondary_dims.iter()) {
            if sec_name == name {
                return Some((sec_name.as_str(), sec_size));
            }
        }

        // If this dimension is indexed, try size-based match
        if ctx.is_indexed_dimension(name) {
            for (sec_name, &sec_size) in secondary_names.iter().zip(secondary_dims.iter()) {
                if ctx.is_indexed_dimension(sec_name) && size == sec_size {
                    return Some((sec_name.as_str(), sec_size));
                }
            }
        }

        None
    }

    /// Compute the size of a range subscript from constant bounds.
    ///
    /// Returns `Some(size)` if both bounds are constant and the range is valid.
    /// Returns `None` in these cases:
    /// - Either bound is not a constant expression (we can't compute at compile time)
    /// - The range is invalid (end < start), which will be caught later during
    ///   compilation when `build_view_from_ops` validates the IndexOp::Range
    ///
    /// When `None` is returned, callers should fall back to the full dimension size
    /// as a conservative upper bound for ArrayBounds.
    fn compute_range_size(start: &Expr2, end: &Expr2, dim: &Dimension) -> Option<usize> {
        let start_idx = Self::expr_to_index(start, dim)?;
        let end_idx = Self::expr_to_index(end, dim)?;
        // Range is inclusive on both ends, so size is end - start + 1
        if end_idx >= start_idx {
            Some(end_idx - start_idx + 1)
        } else {
            None // Invalid range will be caught during build_view_from_ops
        }
    }

    /// Convert an expression to a 0-based index if it's a constant or named element.
    fn expr_to_index(expr: &Expr2, dim: &Dimension) -> Option<usize> {
        match expr {
            Expr2::Const(_, val, _) => {
                // Numeric constant - interpret as 1-based index.
                // Guard against overflow: val must be in range [1, isize::MAX].
                if *val >= 1.0 && *val <= isize::MAX as f64 {
                    Some((*val as usize).saturating_sub(1))
                } else {
                    None
                }
            }
            Expr2::Var(ident, _, _) => {
                // Could be a named dimension element
                if let Dimension::Named(_, named_dim) = dim {
                    named_dim
                        .elements
                        .iter()
                        .position(|elem| elem.as_str() == ident.as_str())
                } else {
                    None
                }
            }
            _ => None,
        }
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
                        name: id.as_str().to_string(),
                        dims: dim_sizes,
                        dim_names: Some(dim_names),
                    })
                } else {
                    None
                };
                Expr2::Var(id, array_bounds, loc)
            }
            Expr1::App(builtin_fn, loc) => {
                use BuiltinFn::*;
                let builtin = match builtin_fn {
                    Lookup(table_expr, index_expr, loc) => Lookup(
                        Box::new(Expr2::from(*table_expr, ctx)?),
                        Box::new(Expr2::from(*index_expr, ctx)?),
                        loc,
                    ),
                    LookupForward(table_expr, index_expr, loc) => LookupForward(
                        Box::new(Expr2::from(*table_expr, ctx)?),
                        Box::new(Expr2::from(*index_expr, ctx)?),
                        loc,
                    ),
                    LookupBackward(table_expr, index_expr, loc) => LookupBackward(
                        Box::new(Expr2::from(*table_expr, ctx)?),
                        Box::new(Expr2::from(*index_expr, ctx)?),
                        loc,
                    ),
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
                    Max(e1, e2) => {
                        // When MAX has a single argument (e2 is None), it behaves as an array
                        // reduction (finding the maximum of all elements in the array),
                        // so we allow cross-dimension unions.
                        let is_array_reduction = e2.is_none();
                        let prev = if is_array_reduction {
                            ctx.set_allow_dimension_union(true)
                        } else {
                            false
                        };
                        let result = Max(
                            Box::new(Expr2::from(*e1, ctx)?),
                            e2.map(|e| Expr2::from(*e, ctx)).transpose()?.map(Box::new),
                        );
                        if is_array_reduction {
                            ctx.set_allow_dimension_union(prev);
                        }
                        result
                    }
                    Mean(exprs) => {
                        // When MEAN has a single argument, it behaves as an array reduction
                        // (averaging all elements in the array), so we allow cross-dimension unions.
                        // With multiple arguments, it's a scalar mean and doesn't need the flag.
                        let is_array_reduction = exprs.len() == 1;
                        let prev = if is_array_reduction {
                            ctx.set_allow_dimension_union(true)
                        } else {
                            false
                        };
                        let exprs: EquationResult<Vec<Expr2>> =
                            exprs.into_iter().map(|e| Expr2::from(e, ctx)).collect();
                        if is_array_reduction {
                            ctx.set_allow_dimension_union(prev);
                        }
                        Mean(exprs?)
                    }
                    Min(e1, e2) => {
                        // When MIN has a single argument (e2 is None), it behaves as an array
                        // reduction (finding the minimum of all elements in the array),
                        // so we allow cross-dimension unions.
                        let is_array_reduction = e2.is_none();
                        let prev = if is_array_reduction {
                            ctx.set_allow_dimension_union(true)
                        } else {
                            false
                        };
                        let result = Min(
                            Box::new(Expr2::from(*e1, ctx)?),
                            e2.map(|e| Expr2::from(*e, ctx)).transpose()?.map(Box::new),
                        );
                        if is_array_reduction {
                            ctx.set_allow_dimension_union(prev);
                        }
                        result
                    }
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
                    Sign(e) => Sign(Box::new(Expr2::from(*e, ctx)?)),
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
                    Size(e) => {
                        // Special case: SIZE(DimensionName) returns the element count of the dimension.
                        // This is used by Vensim's ELMCOUNT function (converted to SIZE in XMILE).
                        //
                        // Note: The XMILE spec (section 3.7.1) states that dimension names "must be
                        // distinct from model variables names within the whole-model." Therefore, we
                        // don't need to disambiguate between a dimension and variable with the same
                        // name - that's an invalid model per the spec. We check dimension names first,
                        // which is the sensible default since SIZE(array_var) can use SIZE(arr[*])
                        // syntax for explicit array sizing.
                        if let Expr1::Var(ref id, loc) = *e
                            && ctx.is_dimension_name(id.as_str())
                        {
                            // Convert SIZE(DimName) to a constant
                            let dim_name = CanonicalDimensionName::from_raw(id.as_str());
                            if let Some(len) = ctx.get_dimension_len(&dim_name) {
                                // Return a constant expression with the dimension size
                                return Ok(Expr2::Const(len.to_string(), len as f64, loc));
                            }
                            // If we can't find the dimension length, fall through to normal processing
                            // which will produce an appropriate error
                        }
                        // Normal case: SIZE(array_expression)
                        // Array reduction builtins allow cross-dimension unions
                        let prev = ctx.set_allow_dimension_union(true);
                        let result = Size(Box::new(Expr2::from(*e, ctx)?));
                        ctx.set_allow_dimension_union(prev);
                        result
                    }
                    Stddev(e) => {
                        // Array reduction builtin - allow cross-dimension unions
                        let prev = ctx.set_allow_dimension_union(true);
                        let result = Stddev(Box::new(Expr2::from(*e, ctx)?));
                        ctx.set_allow_dimension_union(prev);
                        result
                    }
                    Sum(e) => {
                        // Array reduction builtin - allow cross-dimension unions
                        // This enables expressions like SUM(a[*]+h[*]) where a[DimA] and h[DimC]
                        // have different dimensions, producing a cross-product sum.
                        let prev = ctx.set_allow_dimension_union(true);
                        let result = Sum(Box::new(Expr2::from(*e, ctx)?));
                        ctx.set_allow_dimension_union(prev);
                        result
                    }
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
                    let mut result_dim_names = Vec::new();

                    // Simple dimension calculation - count wildcards to determine result dims
                    for (i, arg) in args.iter().enumerate() {
                        if i < dims.len() {
                            match arg {
                                IndexExpr2::Wildcard(_) => {
                                    result_dims.push(dims[i].len());
                                    result_dim_names.push(dims[i].name().to_string());
                                }
                                IndexExpr2::Range(start, end, _) => {
                                    // Try to compute actual range size from constant bounds
                                    let range_size = Self::compute_range_size(start, end, &dims[i]);
                                    result_dims.push(range_size.unwrap_or(dims[i].len()));
                                    result_dim_names.push(dims[i].name().to_string());
                                }
                                IndexExpr2::StarRange(subdim_name, _) => {
                                    // Star ranges use the subdimension's length, not the parent's
                                    // This is critical for correct temp array sizing
                                    if let Some(subdim_len) = ctx.get_dimension_len(subdim_name) {
                                        result_dims.push(subdim_len);
                                        // Use the subdimension name, not the parent dimension
                                        result_dim_names.push(subdim_name.as_str().to_string());
                                    } else {
                                        unreachable!(
                                            "StarRange subdimension '{}' should exist - validated during compilation",
                                            subdim_name.as_str()
                                        );
                                    }
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
                        Some(Self::allocate_temp_array_with_names(
                            ctx,
                            result_dims,
                            result_dim_names,
                        ))
                    }
                } else {
                    None // Scalar variable or unknown variable
                };

                Expr2::Subscript(id, args, array_bounds, loc)
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
            Expr2::Var(v, _, loc) if v.as_str() == ident => Some(*loc),
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
            Expr2::Subscript(v, _args, _, loc) if v.as_str() == ident => Some(*loc),
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
    use crate::common::canonicalize;
    use std::collections::HashMap;
    use std::iter::Iterator;

    // Helper function to create indexed dimensions for testing
    fn indexed_dims(sizes: &[u32]) -> Vec<Dimension> {
        use crate::common::CanonicalDimensionName;
        sizes
            .iter()
            .enumerate()
            .map(|(i, &size)| {
                Dimension::Indexed(CanonicalDimensionName::from_raw(&format!("dim{i}")), size)
            })
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

        fn get_dimension_len(&self, _name: &CanonicalDimensionName) -> Option<usize> {
            // For tests, we don't have dimension context
            None
        }

        fn is_indexed_dimension(&self, _name: &str) -> bool {
            // For tests, treat all dimensions as named (not indexed).
            // This is more conservative - dimensions must match by name, not size.
            // Tests that need indexed dimension behavior should use TestProject
            // which has proper dimension context.
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
        assert!(const_int_eval(&Expr2::Var(canonicalize("foo"), None, Loc::default())).is_err());

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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Test scalar variable (no dimensions)
        let var_expr = Expr1::Var(canonicalize("scalar_var"), Loc::default());
        let expr2 = Expr2::from(var_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Var(id, array_bounds, _) => {
                assert_eq!(id.as_str(), "scalar_var");
                assert!(array_bounds.is_none()); // Scalar has no array bounds
            }
            _ => panic!("Expected Var expression"),
        }
    }

    #[test]
    fn test_expr2_from_array_var() {
        use crate::ast::expr1::Expr1;
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[3, 4]));

        // Test array variable with dimensions
        let var_expr = Expr1::Var(canonicalize("array_var"), Loc::default());
        let expr2 = Expr2::from(var_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Var(id, array_bounds, _) => {
                assert_eq!(id.as_str(), "array_var");
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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("matrix".to_string(), indexed_dims(&[3, 4]));

        // Test subscript with one index reduces dimension
        let subscript_expr = Expr1::Subscript(
            canonicalize("matrix"),
            vec![
                IndexExpr1::Expr(Expr1::Const("1".to_string(), 1.0, Loc::default())),
                IndexExpr1::Wildcard(Loc::default()),
            ],
            Loc::default(),
        );
        let expr2 = Expr2::from(subscript_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Subscript(id, args, array_bounds, _) => {
                assert_eq!(id.as_str(), "matrix");
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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("vector".to_string(), indexed_dims(&[5]));

        // Test subscript that results in scalar
        let subscript_expr = Expr1::Subscript(
            canonicalize("vector"),
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
                assert_eq!(id.as_str(), "vector");
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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 3]));

        // Test unary negative preserves array dimensions
        let neg_expr = Expr1::Op1(
            UnaryOp::Negative,
            Box::new(Expr1::Var(canonicalize("array_var"), Loc::default())),
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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("matrix".to_string(), indexed_dims(&[3, 4]));

        // Test transpose reverses dimensions
        let transpose_expr = Expr1::Op1(
            UnaryOp::Transpose,
            Box::new(Expr1::Var(canonicalize("matrix"), Loc::default())),
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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 3]));

        // Test array + scalar (broadcasting)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(canonicalize("array_var"), Loc::default())),
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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array1".to_string(), indexed_dims(&[3, 4]));
        ctx.dimensions
            .insert("array2".to_string(), indexed_dims(&[3, 4]));

        // Test array + array (matching dimensions)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(canonicalize("array1"), Loc::default())),
            Box::new(Expr1::Var(canonicalize("array2"), Loc::default())),
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
        use crate::common::canonicalize;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 2]));

        // Test if expression with array in both branches
        let if_expr = Expr1::If(
            Box::new(Expr1::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr1::Var(canonicalize("array_var"), Loc::default())),
            Box::new(Expr1::Var(canonicalize("array_var"), Loc::default())),
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
        use crate::ast::expr1::Expr1;
        use crate::ast::{BinaryOp, UnaryOp};
        use crate::common::canonicalize;

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
            Box::new(Expr1::Var(canonicalize("array1"), Loc::default())),
            Loc::default(),
        );
        let expr2_1 = Expr2::from(neg_expr, &mut ctx).unwrap();

        // Second operation: array1 + array2 (should get temp_id 1)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(canonicalize("array1"), Loc::default())),
            Box::new(Expr1::Var(canonicalize("array2"), Loc::default())),
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
