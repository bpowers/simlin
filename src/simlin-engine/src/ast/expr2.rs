// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr1::{Expr1, IndexExpr1};
use crate::ast::literal::Literal;
use crate::builtins::{BuiltinContents, BuiltinFn, Loc, walk_builtin_expr};
use crate::common::{Canonical, CanonicalDimensionName, EquationResult, Ident};
use crate::dimensions::{Axis, AxisRelations, Dimension, match_axes_partial};
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
#[derive(PartialEq, Eq, Clone)]
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
    /// Returns the total number of elements in the array.
    ///
    /// Test-only: no production caller reads an `ArrayBounds`' extent (codegen
    /// goes through `ArrayView`/`VarSizes` instead), so this is gated rather
    /// than shipped.
    #[cfg(test)]
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

/// The bounds slot of a lowered node: `None` on a scalar node, boxed on an
/// arrayed one.
///
/// Boxed, not inline, because the trees carrying it are RETAINED (the
/// per-variable lowering memos, the LTM handle maps) and a bound is `None` on
/// every scalar subexpression: an inline [`ArrayBounds`] -- a name and two
/// `Vec`s -- would be paid by every node for the few that carry one, where a
/// `None` box costs one pointer. Readers go through `get_array_bounds`, a copy
/// of a node clones the slot as it stands, and the box is spelled only where a
/// bound is produced ([`Expr2::from`] and the compiler's bare-reference
/// rewrite).
pub type NodeBounds = Option<Box<ArrayBounds>>;

/// IndexExpr represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
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
                IndexExpr2::StarRange(CanonicalDimensionName::from(&ident), loc)
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

    /// The [`Expr2::strip_loc_and_bounds`] twin for one subscript index.
    pub(crate) fn strip_loc_and_bounds(self) -> Self {
        let loc = Loc::default();
        match self {
            IndexExpr2::Wildcard(_) => IndexExpr2::Wildcard(loc),
            IndexExpr2::StarRange(dim, _) => IndexExpr2::StarRange(dim, loc),
            IndexExpr2::Range(l, r, _) => {
                IndexExpr2::Range(l.strip_loc_and_bounds(), r.strip_loc_and_bounds(), loc)
            }
            IndexExpr2::DimPosition(n, _) => IndexExpr2::DimPosition(n, loc),
            IndexExpr2::Expr(e) => IndexExpr2::Expr(e.strip_loc_and_bounds()),
        }
    }

    #[allow(dead_code)]
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
///
/// `Eq` is derived for the reason spelled out on [`crate::ast::Expr0`]: this is
/// the layer that rides on the lowered-variable memos and
/// `ltm_agg::AggNodesResult`, whose
/// salsa backdating is decided by comparing a memo with its own rebuild, so a
/// field that is not equal to itself defeats it. `Eq` makes that a compile-time
/// property rather than a convention.
#[allow(dead_code)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum Expr2 {
    Const(String, Literal, Loc),
    Var(Ident<Canonical>, NodeBounds, Loc),
    App(BuiltinFn<Expr2>, NodeBounds, Loc),
    Subscript(Ident<Canonical>, Vec<IndexExpr2>, NodeBounds, Loc),
    Op1(UnaryOp, Box<Expr2>, NodeBounds, Loc),
    Op2(BinaryOp, Box<Expr2>, Box<Expr2>, NodeBounds, Loc),
    If(Box<Expr2>, Box<Expr2>, Box<Expr2>, NodeBounds, Loc),
}

/// Context trait for converting Expr1 to Expr2
/// Provides access to variable dimension information and temp ID allocation
pub trait Expr2Context {
    /// Get the dimensions of a variable, or None if it's a scalar
    fn get_dimensions(&self, ident: &str) -> Option<&[Dimension]>;

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

    /// Whether `dim_name` declares a mapping onto `target`.
    ///
    /// [`Expr2Relations`] wraps this as the one declared relation bounds
    /// unification can supply to `dimensions::match_axes_partial`, which is
    /// what pairs `DimD` with `DimA` when `DimD` maps onto it.
    fn has_mapping_to(&self, _dim_name: &str, _target: &str) -> bool {
        false
    }
}

/// The declared relations an [`Expr2Context`] can answer: a mapping between
/// two dimension names, in the direction asked. It reaches its dimension facts
/// through the trait rather than through a [`crate::dimensions::DimensionsContext`],
/// so the indirect correspondences -- a mapping onto a PARENT of the target,
/// and a bare subdimension relation -- are not available here, and are the two
/// a caller that resolves no element could not act on anyway.
struct Expr2Relations<'a, C: Expr2Context>(&'a C);

impl<C: Expr2Context> AxisRelations for Expr2Relations<'_, C> {
    fn maps_to(&self, from: &str, to: &str) -> bool {
        self.0.has_mapping_to(from, to)
    }
}

impl Expr2 {
    /// The expression with every `Loc` zeroed and every [`ArrayBounds`]
    /// annotation dropped -- its position- and lowering-independent form.
    ///
    /// Both stripped fields are artifacts of *where* an expression was
    /// written rather than *what* it means: a `Loc` is a byte range into one
    /// variable's equation text, and a `Temp` bound carries a temp id the
    /// lowering context handed out in equation order. Two occurrences of the
    /// same subexpression in different equations therefore differ in both
    /// while denoting the same thing, so a cache that stores an expression
    /// keyed on its canonical printed form (`ltm_agg::AggNode`) must store
    /// this form or its value stops being a function of its key.
    ///
    /// Necessary, not sufficient: `Expr2::Const` holds an `f64`, whose `==` is
    /// not reflexive on NaN, so a NaN-bearing expression compares unequal to
    /// itself however it is normalized. See `ltm_agg::AggNode::reducer` for
    /// that residual and the root fix it waits on.
    pub(crate) fn strip_loc_and_bounds(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr2::Const(text, value, _) => Expr2::Const(text, value, loc),
            Expr2::Var(ident, _, _) => Expr2::Var(ident, None, loc),
            Expr2::App(builtin, _, _) => Expr2::App(
                builtin
                    .map(|arg| arg.strip_loc_and_bounds())
                    .strip_own_locs(),
                None,
                loc,
            ),
            Expr2::Subscript(ident, indices, _, _) => Expr2::Subscript(
                ident,
                indices
                    .into_iter()
                    .map(IndexExpr2::strip_loc_and_bounds)
                    .collect(),
                None,
                loc,
            ),
            Expr2::Op1(op, rhs, _, _) => {
                Expr2::Op1(op, Box::new(rhs.strip_loc_and_bounds()), None, loc)
            }
            Expr2::Op2(op, lhs, rhs, _, _) => Expr2::Op2(
                op,
                Box::new(lhs.strip_loc_and_bounds()),
                Box::new(rhs.strip_loc_and_bounds()),
                None,
                loc,
            ),
            Expr2::If(cond, then_e, else_e, _, _) => Expr2::If(
                Box::new(cond.strip_loc_and_bounds()),
                Box::new(then_e.strip_loc_and_bounds()),
                Box::new(else_e.strip_loc_and_bounds()),
                None,
                loc,
            ),
        }
    }

    /// The array bounds of this node: `None` on a scalar node, always on a
    /// `Const`.
    pub(crate) fn get_array_bounds(&self) -> Option<&ArrayBounds> {
        match self {
            Expr2::Const(_, _, _) => None,
            Expr2::Var(_, array_bounds, _) => array_bounds.as_deref(),
            Expr2::App(_, array_bounds, _) => array_bounds.as_deref(),
            Expr2::Subscript(_, _, array_bounds, _) => array_bounds.as_deref(),
            Expr2::Op1(_, _, array_bounds, _) => array_bounds.as_deref(),
            Expr2::Op2(_, _, _, array_bounds, _) => array_bounds.as_deref(),
            Expr2::If(_, _, _, array_bounds, _) => array_bounds.as_deref(),
        }
    }

    /// Allocates a new temp ID for an array with given dimensions
    fn allocate_temp_array<C: Expr2Context>(ctx: &mut C, dims: Vec<usize>) -> Box<ArrayBounds> {
        Box::new(ArrayBounds::Temp {
            id: ctx.allocate_temp_id(),
            dims,
            dim_names: None, // Temp arrays don't have dimension names initially
        })
    }

    /// Allocates a new temp ID for an array with given dimensions and names
    fn allocate_temp_array_with_names<C: Expr2Context>(
        ctx: &mut C,
        dims: Vec<usize>,
        names: Vec<String>,
    ) -> Box<ArrayBounds> {
        Box::new(ArrayBounds::Temp {
            id: ctx.allocate_temp_id(),
            dims,
            dim_names: Some(names),
        })
    }

    fn unify_array_bounds<C: Expr2Context>(
        ctx: &mut C,
        l: Option<&ArrayBounds>,
        r: Option<&ArrayBounds>,
        loc: Loc,
    ) -> EquationResult<NodeBounds> {
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

    /// Unify two operands' array bounds: the result's dimensions and their
    /// names, or `MismatchedDimensions`.
    ///
    /// Axes are paired by [`crate::dimensions::match_axes_partial`], the
    /// engine's one axis-matching precedence, over the axes an `ArrayBounds`
    /// carries -- each a name, a length, and whether its dimension is indexed.
    /// [`Expr2Relations`] is the projection this position can supply: a
    /// declared mapping between two dimension names, and nothing else.
    ///
    /// Three outcomes follow from the pairing:
    ///
    /// - one side's axes all pair into the other's: the wider operand's axis
    ///   ORDER is the result's, and every paired axis must agree in length;
    /// - neither side's axes all pair: the result is the UNION, which is
    ///   allowed only while every unpaired axis is an indexed dimension --
    ///   `Cities` and `Products` are not interchangeable because both hold two
    ///   elements -- or while an enclosing array-reduction builtin has opened
    ///   the union gate, where a cross-dimension expression is what `SUM(a[*] +
    ///   h[*])` means;
    /// - either side is unnamed: neither can be paired by name, so the two
    ///   lists must agree position for position ([`Self::unify_dims`]).
    fn unify_dims_with_names<C: Expr2Context>(
        ctx: &C,
        a_dims: &[usize],
        a_names: Option<&[String]>,
        b_dims: &[usize],
        b_names: Option<&[String]>,
        loc: Loc,
    ) -> EquationResult<(Vec<usize>, Option<Vec<String>>)> {
        // Without names on both sides there is nothing to pair by, so the
        // lists must agree position for position.
        let (a_names, b_names) = match (a_names, b_names) {
            (Some(a), Some(b)) => (a, b),
            _ => {
                let dims = Self::unify_dims(a_dims, b_dims, loc)?;
                return Ok((dims, None));
            }
        };

        let a_axes = Self::bound_axes(ctx, a_names, a_dims);
        let b_axes = Self::bound_axes(ctx, b_names, b_dims);
        let relations = Expr2Relations(ctx);

        // The pairing is ONE-TO-ONE, which is what makes "can every axis of
        // this side be supplied by the other" a question about the whole list:
        // for `a[X(3), Y(3)]` against `b[Z(3)]`, searching per axis lets both
        // X and Y claim Z and reports a match that does not exist.
        let a_to_b = match_axes_partial(&a_axes, &b_axes, &relations);
        let b_to_a = match_axes_partial(&b_axes, &a_axes, &relations);
        let a_can_match_b = a_to_b.iter().all(Option::is_some);
        let b_can_match_a = b_to_a.iter().all(Option::is_some);

        if !a_can_match_b && !b_can_match_a {
            let allow_union = ctx.allow_dimension_union();
            for (axis, matched) in a_axes.iter().zip(a_to_b.iter()) {
                if matched.is_none() && !axis.indexed && !allow_union {
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }
            }
            for (axis, matched) in b_axes.iter().zip(b_to_a.iter()) {
                if matched.is_none() && !axis.indexed && !allow_union {
                    return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                }
            }

            // All of a's axes, then b's unpaired ones.
            let mut unified_dims: Vec<usize> = a_axes.iter().map(|axis| axis.len).collect();
            let mut unified_names: Vec<String> = a_names.to_vec();
            for (axis, matched) in b_axes.iter().zip(b_to_a.iter()) {
                match matched {
                    // Already present through a's axis; the two must agree in
                    // length, which a name match does not itself guarantee
                    // (a range-derived axis keeps its parent's name at a
                    // smaller size).
                    Some((a_idx, _)) => {
                        if axis.len != a_axes[*a_idx].len {
                            return eqn_err!(MismatchedDimensions, loc.start, loc.end);
                        }
                    }
                    None => {
                        unified_dims.push(axis.len);
                        unified_names.push(axis.name.to_string());
                    }
                }
            }

            return Ok((unified_dims, Some(unified_names)));
        }

        // One side's axes all pair into the other's: the wider operand gives
        // the result its axis order, and a's order breaks a tie.
        let (primary, secondary, primary_to_secondary) =
            if b_can_match_a || a_axes.len() >= b_axes.len() {
                (&a_axes, &b_axes, &a_to_b)
            } else {
                (&b_axes, &a_axes, &b_to_a)
            };

        let mut unified_dims = Vec::with_capacity(primary.len());
        let mut unified_names = Vec::with_capacity(primary.len());
        for (axis, matched) in primary.iter().zip(primary_to_secondary.iter()) {
            if let Some((secondary_idx, _)) = matched
                && axis.len != secondary[*secondary_idx].len
            {
                return eqn_err!(MismatchedDimensions, loc.start, loc.end);
            }
            unified_dims.push(axis.len);
            unified_names.push(axis.name.to_string());
        }

        Ok((unified_dims, Some(unified_names)))
    }

    /// The axes an `ArrayBounds`'s parallel name and length lists describe.
    fn bound_axes<'n, C: Expr2Context>(
        ctx: &C,
        names: &'n [String],
        dims: &[usize],
    ) -> Vec<Axis<'n>> {
        names
            .iter()
            .zip(dims.iter())
            .map(|(name, &len)| Axis {
                name: name.as_str(),
                len,
                indexed: ctx.is_indexed_dimension(name),
            })
            .collect()
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
                let val = val.value();
                if val >= 1.0 && val <= isize::MAX as f64 {
                    Some((val as usize).saturating_sub(1))
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
                    Some(Box::new(ArrayBounds::Named {
                        name: id.as_str().to_string(),
                        dims: dim_sizes,
                        dim_names: Some(dim_names),
                    }))
                } else {
                    None
                };
                Expr2::Var(id, array_bounds, loc)
            }
            Expr1::App(builtin_fn, loc) => {
                // SIZE(DimensionName) returns the element count of the dimension.
                // This is used by Vensim's ELMCOUNT function (converted to SIZE in XMILE).
                //
                // Note: The XMILE spec (section 3.7.1) states that dimension names "must be
                // distinct from model variables names within the whole-model." Therefore, we
                // don't need to disambiguate between a dimension and variable with the same
                // name - that's an invalid model per the spec. We check dimension names first,
                // which is the sensible default since SIZE(array_var) can use SIZE(arr[*])
                // syntax for explicit array sizing. A dimension whose length the context
                // cannot report falls through to normal processing, which produces the
                // appropriate error.
                if let BuiltinFn::Size(arg) = &builtin_fn
                    && let Expr1::Var(id, var_loc) = &**arg
                    && ctx.is_dimension_name(id.as_str())
                    && let Some(len) = ctx.get_dimension_len(&CanonicalDimensionName::from(id))
                {
                    return Ok(Expr2::Const(
                        len.to_string(),
                        Literal::new(len as f64),
                        *var_loc,
                    ));
                }
                // Inside an array operand (`ArgKind::Array`: the reducers' and
                // the vector builtins' array positions) sub-expressions may
                // union disjoint named dimensions -- `SUM(a[*] + h[*])` over
                // `a[DimA]` and `h[DimC]` is a cross-product sum -- so the
                // union gate is open while such a call's arguments lower.
                let allow_union = builtin_fn.has_array_operand();
                let prev = allow_union.then(|| ctx.set_allow_dimension_union(true));
                let lowered = builtin_fn.try_map(|e| Expr2::from(e, ctx));
                if let Some(prev) = prev {
                    ctx.set_allow_dimension_union(prev);
                }
                // TODO: Handle array sources for builtin functions that return arrays
                Expr2::App(lowered?, None, loc)
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
                        // Transpose reverses both dimensions and dimension names.
                        // Preserving names is critical: when this expression is
                        // materialized into a temp (`compiler::array_operand`),
                        // the temp view's dim_ids must match the source view's
                        // transposed dim_ids for the VM's LoadIterViewAt
                        // dimension matching to succeed.
                        let mut transposed_dims = bounds.dims().to_vec();
                        transposed_dims.reverse();
                        if let Some(names) = bounds.dim_names() {
                            let mut transposed_names = names.to_vec();
                            transposed_names.reverse();
                            Some(Self::allocate_temp_array_with_names(
                                ctx,
                                transposed_dims,
                                transposed_names,
                            ))
                        } else {
                            Some(Self::allocate_temp_array(ctx, transposed_dims))
                        }
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

                // Compute array bounds for if expressions.
                // First try to unify the then/else branch bounds.
                let branch_bounds = Self::unify_array_bounds(
                    ctx,
                    t_expr.get_array_bounds(),
                    f_expr.get_array_bounds(),
                    loc,
                )?;

                // If the branches are both scalar but the condition is
                // array-valued, the IF expression should inherit the
                // condition's dimensions (broadcasting scalar branches).
                let array_bounds = branch_bounds
                    .or_else(|| cond_expr.get_array_bounds().map(|b| Box::new(b.clone())));

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

    #[allow(dead_code)]
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
                    // The lookup table identity is a locatable reference too.
                    BuiltinContents::Expr(expr) | BuiltinContents::LookupTable(expr) => {
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

#[cfg(test)]
mod tests {
    use super::*;
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
        fn get_dimensions(&self, ident: &str) -> Option<&[Dimension]> {
            self.dimensions.get(ident).map(|dims| dims.as_slice())
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
        assert_eq!(named_bounds.dims(), &[3, 4]);
        assert_eq!(named_bounds.size(), 12); // 3 * 4 = 12

        // Test Temp variant
        let temp_bounds = ArrayBounds::Temp {
            id: 5,
            dims: vec![2, 3],
            dim_names: None,
        };
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
    fn test_expr2_from_scalar_var() {
        use crate::ast::expr1::Expr1;
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Test scalar variable (no dimensions)
        let var_expr = Expr1::Var(Ident::new("scalar_var"), Loc::default());
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[3, 4]));

        // Test array variable with dimensions
        let var_expr = Expr1::Var(Ident::new("array_var"), Loc::default());
        let expr2 = Expr2::from(var_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Var(id, array_bounds, _) => {
                assert_eq!(id.as_str(), "array_var");
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match *bounds {
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("matrix".to_string(), indexed_dims(&[3, 4]));

        // Test subscript with one index reduces dimension
        let subscript_expr = Expr1::Subscript(
            Ident::new("matrix"),
            vec![
                IndexExpr1::Expr(Expr1::Const(
                    "1".to_string(),
                    Literal::new(1.0),
                    Loc::default(),
                )),
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
                match *bounds {
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context for array variable
        ctx.dimensions
            .insert("vector".to_string(), indexed_dims(&[5]));

        // Test subscript that results in scalar
        let subscript_expr = Expr1::Subscript(
            Ident::new("vector"),
            vec![IndexExpr1::Expr(Expr1::Const(
                "2".to_string(),
                Literal::new(2.0),
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 3]));

        // Test unary negative preserves array dimensions
        let neg_expr = Expr1::Op1(
            UnaryOp::Negative,
            Box::new(Expr1::Var(Ident::new("array_var"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(neg_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op1(UnaryOp::Negative, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match *bounds {
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("matrix".to_string(), indexed_dims(&[3, 4]));

        // Test transpose reverses dimensions
        let transpose_expr = Expr1::Op1(
            UnaryOp::Transpose,
            Box::new(Expr1::Var(Ident::new("matrix"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(transpose_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op1(UnaryOp::Transpose, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match *bounds {
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 3]));

        // Test array + scalar (broadcasting)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(Ident::new("array_var"), Loc::default())),
            Box::new(Expr1::Const(
                "10".to_string(),
                Literal::new(10.0),
                Loc::default(),
            )),
            Loc::default(),
        );
        let expr2 = Expr2::from(add_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op2(BinaryOp::Add, _, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match *bounds {
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array1".to_string(), indexed_dims(&[3, 4]));
        ctx.dimensions
            .insert("array2".to_string(), indexed_dims(&[3, 4]));

        // Test array + array (matching dimensions)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(Ident::new("array1"), Loc::default())),
            Box::new(Expr1::Var(Ident::new("array2"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(add_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op2(BinaryOp::Add, _, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match *bounds {
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
        use crate::common::Ident;

        let mut ctx = TestContext::new();

        // Add dimensions to context
        ctx.dimensions
            .insert("array_var".to_string(), indexed_dims(&[2, 2]));

        // Test if expression with array in both branches
        let if_expr = Expr1::If(
            Box::new(Expr1::Const(
                "1".to_string(),
                Literal::new(1.0),
                Loc::default(),
            )),
            Box::new(Expr1::Var(Ident::new("array_var"), Loc::default())),
            Box::new(Expr1::Var(Ident::new("array_var"), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(if_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::If(_, _, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                let bounds = array_bounds.unwrap();
                match *bounds {
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
        use crate::common::Ident;

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
            Box::new(Expr1::Var(Ident::new("array1"), Loc::default())),
            Loc::default(),
        );
        let expr2_1 = Expr2::from(neg_expr, &mut ctx).unwrap();

        // Second operation: array1 + array2 (should get temp_id 1)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(Ident::new("array1"), Loc::default())),
            Box::new(Expr1::Var(Ident::new("array2"), Loc::default())),
            Loc::default(),
        );
        let expr2_2 = Expr2::from(add_expr, &mut ctx).unwrap();

        // Check first operation got temp_id 0
        match expr2_1 {
            Expr2::Op1(_, _, array_bounds, _) => {
                assert!(array_bounds.is_some());
                match *array_bounds.unwrap() {
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
                match *array_bounds.unwrap() {
                    ArrayBounds::Temp { id, .. } => assert_eq!(id, 1),
                    _ => panic!("Expected Temp array bounds"),
                }
            }
            _ => panic!("Expected Op2"),
        }
    }
}
