// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::array_view::ArrayView;
use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr2::{ArrayBounds, Expr2, IndexExpr2};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, EquationResult, Ident,
};
use crate::dimensions::Dimension;
use crate::eqn_err;

/// Index expression for Expr3 subscripts.
///
/// Unlike IndexExpr2, this type does NOT have a Wildcard variant.
/// During the expr2 → expr3 lowering pass, all wildcards are resolved
/// to explicit StarRange expressions based on the variable's dimensions.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr3 {
    /// Star range (*:dim or dim.*) - preserves dimension for iteration.
    /// This includes both user-specified star ranges AND wildcards that
    /// were converted during lowering.
    StarRange(CanonicalDimensionName, Loc),
    /// Range subscript (e.g., 1:3 or Boston:LA)
    Range(Expr3, Expr3, Loc),
    /// Dimension position reference (e.g., @1, @2)
    DimPosition(u32, Loc),
    /// General expression subscript
    Expr(Expr3),
    /// Active A2A dimension reference.
    /// When a dimension name appears as a subscript in an A2A context,
    /// the value depends on which A2A element is being evaluated.
    /// This variant makes it explicit that pass 2 (not pass 1) must handle it.
    Dimension(CanonicalDimensionName, Loc),
}

impl IndexExpr3 {
    #[allow(dead_code)] // Used in pass 2
    pub fn get_loc(&self) -> Loc {
        match self {
            IndexExpr3::StarRange(_, loc) => *loc,
            IndexExpr3::Range(_, _, loc) => *loc,
            IndexExpr3::DimPosition(_, loc) => *loc,
            IndexExpr3::Expr(e) => e.get_loc(),
            IndexExpr3::Dimension(_, loc) => *loc,
        }
    }

    /// Returns true if this index expression references an A2A dimension.
    /// Such expressions cannot be fully resolved until pass 2 when we know
    /// which specific A2A element is being evaluated.
    #[allow(dead_code)] // Used in pass 2
    pub fn references_a2a_dimension(&self) -> bool {
        match self {
            IndexExpr3::Dimension(_, _) => true,
            IndexExpr3::DimPosition(_, _) => true,
            IndexExpr3::Range(start, end, _) => {
                start.references_a2a_dimension() || end.references_a2a_dimension()
            }
            IndexExpr3::Expr(e) => e.references_a2a_dimension(),
            IndexExpr3::StarRange(_, _) => false,
        }
    }
}

/// Expr3 is the intermediate expression representation between type-checked Expr2
/// and the final compiler::Expr.
///
/// Key differences from Expr2:
/// - Adds array-specific variants: StaticSubscript, TempArray, TempArrayElement, AssignTemp
/// - StaticSubscript includes precomputed ArrayView for efficient array access
/// - TempArray/AssignTemp support temporary array storage for complex expressions
///
/// Key differences from compiler::Expr:
/// - Uses Ident<Canonical> for variable names (not usize offsets)
/// - Keeps string representation in Const for debugging
/// - No module-specific variants (EvalModule, ModuleInput)
/// - No assignment variants (AssignCurr, AssignNext)
#[derive(PartialEq, Clone, Debug)]
pub enum Expr3 {
    // Core variants (similar to Expr2)
    Const(String, f64, Loc),
    Var(Ident<Canonical>, Option<ArrayBounds>, Loc),
    App(BuiltinFn<Expr3>, Option<ArrayBounds>, Loc),
    /// Dynamic subscript - indices computed at runtime
    Subscript(Ident<Canonical>, Vec<IndexExpr3>, Option<ArrayBounds>, Loc),
    Op1(UnaryOp, Box<Expr3>, Option<ArrayBounds>, Loc),
    Op2(BinaryOp, Box<Expr3>, Box<Expr3>, Option<ArrayBounds>, Loc),
    If(Box<Expr3>, Box<Expr3>, Box<Expr3>, Option<ArrayBounds>, Loc),

    // Array-specific variants (some used in pass 2)
    /// Static subscript with precomputed view.
    /// (variable name, view into array, base offset of variable, location)
    #[allow(dead_code)] // Used in pass 2
    StaticSubscript(Ident<Canonical>, ArrayView, usize, Loc),
    /// Reference to a temporary array
    TempArray(u32, ArrayView, Loc),
    /// Reference to a specific element of a temporary array
    #[allow(dead_code)] // Used in pass 2
    TempArrayElement(u32, ArrayView, usize, Loc),
    /// Assign an expression result to temporary array storage
    AssignTemp(u32, Box<Expr3>, ArrayView),
}

impl Expr3 {
    pub fn get_loc(&self) -> Loc {
        match self {
            Expr3::Const(_, _, loc) => *loc,
            Expr3::Var(_, _, loc) => *loc,
            Expr3::App(_, _, loc) => *loc,
            Expr3::Subscript(_, _, _, loc) => *loc,
            Expr3::Op1(_, _, _, loc) => *loc,
            Expr3::Op2(_, _, _, _, loc) => *loc,
            Expr3::If(_, _, _, _, loc) => *loc,
            Expr3::StaticSubscript(_, _, _, loc) => *loc,
            Expr3::TempArray(_, _, loc) => *loc,
            Expr3::TempArrayElement(_, _, _, loc) => *loc,
            Expr3::AssignTemp(_, _, _) => Loc::default(),
        }
    }

    pub fn get_array_bounds(&self) -> Option<&ArrayBounds> {
        match self {
            Expr3::Const(_, _, _) => None,
            Expr3::Var(_, bounds, _) => bounds.as_ref(),
            Expr3::App(_, bounds, _) => bounds.as_ref(),
            Expr3::Subscript(_, _, bounds, _) => bounds.as_ref(),
            Expr3::Op1(_, _, bounds, _) => bounds.as_ref(),
            Expr3::Op2(_, _, _, bounds, _) => bounds.as_ref(),
            Expr3::If(_, _, _, bounds, _) => bounds.as_ref(),
            // Array-specific variants encode their dimensions in ArrayView, not ArrayBounds
            Expr3::StaticSubscript(_, _, _, _) => None,
            Expr3::TempArray(_, _, _) => None,
            Expr3::TempArrayElement(_, _, _, _) => None,
            Expr3::AssignTemp(_, _, _) => None,
        }
    }

    /// Get the ArrayView for array-specific variants, if present
    pub fn get_array_view(&self) -> Option<&ArrayView> {
        match self {
            Expr3::StaticSubscript(_, view, _, _) => Some(view),
            Expr3::TempArray(_, view, _) => Some(view),
            Expr3::TempArrayElement(_, view, _, _) => Some(view),
            Expr3::AssignTemp(_, _, view) => Some(view),
            _ => None,
        }
    }

    /// Returns true if this expression contains any A2A dimension references.
    /// Such expressions cannot be fully resolved until pass 2 when we know
    /// which specific A2A element is being evaluated.
    #[allow(dead_code)] // Used in pass 2
    pub fn references_a2a_dimension(&self) -> bool {
        match self {
            Expr3::Const(_, _, _) => false,
            Expr3::Var(_, _, _) => false,
            Expr3::App(builtin, _, _) => {
                use crate::builtins::BuiltinFn::*;
                match builtin {
                    Lookup(_, e, _)
                    | Abs(e)
                    | Arccos(e)
                    | Arcsin(e)
                    | Arctan(e)
                    | Cos(e)
                    | Exp(e)
                    | Int(e)
                    | Ln(e)
                    | Log10(e)
                    | Sign(e)
                    | Sin(e)
                    | Sqrt(e)
                    | Tan(e)
                    | Size(e)
                    | Stddev(e)
                    | Sum(e) => e.references_a2a_dimension(),
                    Max(a, b) | Min(a, b) => {
                        a.references_a2a_dimension()
                            || b.as_ref().is_some_and(|e| e.references_a2a_dimension())
                    }
                    Mean(exprs) => exprs.iter().any(|e| e.references_a2a_dimension()),
                    Pulse(a, b, c) | Ramp(a, b, c) | SafeDiv(a, b, c) => {
                        a.references_a2a_dimension()
                            || b.references_a2a_dimension()
                            || c.as_ref().is_some_and(|e| e.references_a2a_dimension())
                    }
                    Step(a, b) => a.references_a2a_dimension() || b.references_a2a_dimension(),
                    Rank(e, opt) => {
                        e.references_a2a_dimension()
                            || opt.as_ref().is_some_and(|(a, b)| {
                                a.references_a2a_dimension()
                                    || b.as_ref().is_some_and(|e| e.references_a2a_dimension())
                            })
                    }
                    Inf | Pi | Time | TimeStep | StartTime | FinalTime | IsModuleInput(_, _) => {
                        false
                    }
                }
            }
            Expr3::Subscript(_, indices, _, _) => {
                indices.iter().any(|idx| idx.references_a2a_dimension())
            }
            Expr3::Op1(_, inner, _, _) => inner.references_a2a_dimension(),
            Expr3::Op2(_, left, right, _, _) => {
                left.references_a2a_dimension() || right.references_a2a_dimension()
            }
            Expr3::If(cond, then_expr, else_expr, _, _) => {
                cond.references_a2a_dimension()
                    || then_expr.references_a2a_dimension()
                    || else_expr.references_a2a_dimension()
            }
            // Array-specific variants don't contain A2A dimension references
            // (they've already been processed)
            Expr3::StaticSubscript(_, _, _, _)
            | Expr3::TempArray(_, _, _)
            | Expr3::TempArrayElement(_, _, _, _) => false,
            Expr3::AssignTemp(_, expr, _) => expr.references_a2a_dimension(),
        }
    }
}

// ============================================================================
// Expr2 → Expr3 Lowering (Pass 0)
// ============================================================================
//
// This lowering pass performs:
// 1. Wildcard resolution: Converts `*` to `*:dim` based on variable dimensions
// 2. Bare array expansion: Adds implicit subscripts to bare array references
//    (e.g., `revenue` becomes `revenue[Location, Product]`)
//
// After this pass, all array subscripts are explicit and wildcards are resolved.

/// Context trait for converting Expr2 to Expr3.
///
/// Provides access to variable dimension information needed for:
/// - Resolving wildcards to explicit star ranges
/// - Adding implicit subscripts to bare array references
/// - Detecting dimension name references in subscripts
pub trait Expr3LowerContext {
    /// Get the dimensions of a variable, or None if it's a scalar.
    fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>>;

    /// Check if an identifier is a dimension name (not a variable).
    /// Used to detect A2A dimension references in subscripts.
    fn is_dimension_name(&self, ident: &str) -> bool;
}

impl IndexExpr3 {
    /// Lower an IndexExpr2 to IndexExpr3, resolving wildcards to star ranges.
    ///
    /// # Arguments
    /// * `expr` - The IndexExpr2 to lower
    /// * `dim` - The dimension at this subscript position (None if out of bounds)
    /// * `ctx` - Context for lowering nested expressions
    ///
    /// # Errors
    /// Returns an error if a wildcard is used but no dimension is available
    /// (e.g., subscripting a scalar variable or out-of-bounds subscript).
    pub fn from_index_expr2<C: Expr3LowerContext>(
        expr: &IndexExpr2,
        dim: Option<&Dimension>,
        ctx: &C,
    ) -> EquationResult<Self> {
        match expr {
            IndexExpr2::Wildcard(loc) => {
                // Wildcard must be resolved to the dimension at this position.
                // Note: dim is None when either:
                // 1. The variable is a scalar (CantSubscriptScalar)
                // 2. The subscript position exceeds the dimension count (caught by caller)
                let dim = dim.ok_or(crate::common::EquationError {
                    start: loc.start,
                    end: loc.end,
                    code: crate::common::ErrorCode::CantSubscriptScalar,
                })?;
                // Convert wildcard to star range with the parent dimension name.
                // For indexed dimensions like Dim(5), this becomes StarRange("dim").
                // For named dimensions like Cities{Boston,NYC,LA}, this becomes StarRange("cities").
                // The downstream compiler/evaluator must recognize that StarRange(parent_dim)
                // means "iterate over all elements" (equivalent to IndexOp::Wildcard).
                let dim_name = CanonicalDimensionName::from_raw(dim.name());
                Ok(IndexExpr3::StarRange(dim_name, *loc))
            }
            IndexExpr2::StarRange(subdim_name, loc) => {
                // Explicit star range - pass through unchanged
                Ok(IndexExpr3::StarRange(subdim_name.clone(), *loc))
            }
            IndexExpr2::Range(start, end, loc) => {
                let start_expr = Expr3::from_expr2(start, ctx)?;
                let end_expr = Expr3::from_expr2(end, ctx)?;
                Ok(IndexExpr3::Range(start_expr, end_expr, *loc))
            }
            IndexExpr2::DimPosition(pos, loc) => Ok(IndexExpr3::DimPosition(*pos, *loc)),
            IndexExpr2::Expr(e) => {
                // Check if this is a bare variable that matches a dimension name.
                // This indicates an A2A (apply-to-all) dimension reference.
                //
                // IMPORTANT: If the parent dimension contains this name as an element,
                // it should be treated as an element reference, not a dimension reference.
                // Element names take precedence over dimension names in subscript context.
                if let Expr2::Var(ident, None, loc) = e
                    && ctx.is_dimension_name(ident.as_str())
                {
                    // Check if this is an element of the parent dimension first
                    let element_name = CanonicalElementName::from_raw(ident.as_str());
                    let is_element_of_parent = dim
                        .map(|d| d.get_offset(&element_name).is_some())
                        .unwrap_or(false);

                    if !is_element_of_parent {
                        let canonical = CanonicalDimensionName::from_raw(ident.as_str());
                        return Ok(IndexExpr3::Dimension(canonical, *loc));
                    }
                }
                let expr3 = Expr3::from_expr2(e, ctx)?;
                Ok(IndexExpr3::Expr(expr3))
            }
        }
    }
}

impl Expr3 {
    /// Lower an Expr2 to Expr3, performing pass 0 transformations:
    /// - Resolve wildcards to explicit star ranges
    /// - Add implicit subscripts to bare array references
    ///
    /// # Errors
    /// Returns an error if:
    /// - A wildcard is used on a non-arrayed variable
    /// - A subscript is applied to a scalar variable
    pub fn from_expr2<C: Expr3LowerContext>(expr: &Expr2, ctx: &C) -> EquationResult<Self> {
        match expr {
            Expr2::Const(s, n, loc) => Ok(Expr3::Const(s.clone(), *n, *loc)),

            Expr2::Var(id, bounds, loc) => {
                // Check if this is an array variable that needs implicit subscripts
                if let Some(dims) = ctx.get_dimensions(id.as_str())
                    && !dims.is_empty()
                {
                    // This is a bare array reference - add implicit wildcards
                    // which are immediately resolved to star ranges
                    let subscripts: Vec<IndexExpr3> = dims
                        .iter()
                        .map(|dim| {
                            let dim_name = CanonicalDimensionName::from_raw(dim.name());
                            IndexExpr3::StarRange(dim_name, *loc)
                        })
                        .collect();

                    return Ok(Expr3::Subscript(
                        id.clone(),
                        subscripts,
                        bounds.clone(),
                        *loc,
                    ));
                }
                // Scalar variable or unknown - pass through as-is
                Ok(Expr3::Var(id.clone(), bounds.clone(), *loc))
            }

            Expr2::App(builtin, bounds, loc) => {
                let lowered_builtin = builtin.clone().try_map(|e| Expr3::from_expr2(&e, ctx))?;
                Ok(Expr3::App(lowered_builtin, bounds.clone(), *loc))
            }

            Expr2::Subscript(id, args, bounds, loc) => {
                // Get dimensions for this variable to resolve wildcards
                let dims = ctx.get_dimensions(id.as_str());

                // Check if subscripting a scalar (no dimensions or empty dimensions)
                let is_scalar = dims.as_ref().is_none_or(|d| d.is_empty());
                if is_scalar {
                    // Subscripting a scalar - check if any wildcards
                    for arg in args {
                        if let IndexExpr2::Wildcard(wloc) = arg {
                            return eqn_err!(CantSubscriptScalar, wloc.start, wloc.end);
                        }
                    }
                }

                // Validate subscript count matches dimension count.
                // This catches cases like arr[*, *, *] on a 2D array before
                // we hit misleading errors in individual subscript lowering.
                if let Some(ref d) = dims
                    && args.len() > d.len()
                {
                    // Find the first out-of-bounds subscript for error location
                    let first_extra = &args[d.len()];
                    let extra_loc = first_extra.get_loc();
                    return eqn_err!(MismatchedDimensions, extra_loc.start, extra_loc.end);
                }

                let dims_ref = dims.as_deref();
                let lowered_args: EquationResult<Vec<IndexExpr3>> = args
                    .iter()
                    .enumerate()
                    .map(|(i, arg)| {
                        let dim = dims_ref.and_then(|d| d.get(i));
                        IndexExpr3::from_index_expr2(arg, dim, ctx)
                    })
                    .collect();

                Ok(Expr3::Subscript(
                    id.clone(),
                    lowered_args?,
                    bounds.clone(),
                    *loc,
                ))
            }

            Expr2::Op1(op, inner, bounds, loc) => {
                let inner_expr = Expr3::from_expr2(inner, ctx)?;
                Ok(Expr3::Op1(*op, Box::new(inner_expr), bounds.clone(), *loc))
            }

            Expr2::Op2(op, left, right, bounds, loc) => {
                let left_expr = Expr3::from_expr2(left, ctx)?;
                let right_expr = Expr3::from_expr2(right, ctx)?;
                Ok(Expr3::Op2(
                    *op,
                    Box::new(left_expr),
                    Box::new(right_expr),
                    bounds.clone(),
                    *loc,
                ))
            }

            Expr2::If(cond, then_expr, else_expr, bounds, loc) => {
                let cond_expr = Expr3::from_expr2(cond, ctx)?;
                let then_expr = Expr3::from_expr2(then_expr, ctx)?;
                let else_expr = Expr3::from_expr2(else_expr, ctx)?;
                Ok(Expr3::If(
                    Box::new(cond_expr),
                    Box::new(then_expr),
                    Box::new(else_expr),
                    bounds.clone(),
                    *loc,
                ))
            }
        }
    }
}

// ============================================================================
// Pass 1: Temp Array Decomposition (Expr3 → Expr3)
// ============================================================================
//
// Pass 1 processes Expr3 trees and decomposes complex expressions inside
// array builtins (SUM, MEAN, MIN, MAX, STDDEV, SIZE) into temporary arrays.
//
// Example transformation:
//   SUM(source[3:5] + 1)
//   → AssignTemp(0, source[3:5] + 1, view)
//     SUM(TempArray(0, view))
//
// Pass 1 only handles expressions that do NOT contain A2A dimension references.
// Expressions with dimension references (like arr[*, DimName]) are deferred
// to pass 2, which runs per-A2A-element.
//
// The transformation is applied recursively bottom-up to handle nested cases:
//   SUM(SUM(inner) + outer)
//   → (inner SUM evaluated first, then outer decomposed)

/// Context for pass 1 temp decomposition.
/// Tracks temp ID allocation across the transformation.
///
/// When A2A context is provided (active_dimensions and active_subscripts),
/// Dimension and DimPosition references can be resolved to concrete values,
/// enabling decomposition of expressions that would otherwise be deferred.
pub struct Pass1Context<'a> {
    /// Counter for allocating temp array IDs
    next_temp_id: u32,
    /// Accumulated AssignTemp expressions (prepended to result)
    temp_assignments: Vec<Expr3>,
    /// Active dimensions for A2A context (None if not in A2A evaluation)
    active_dimensions: Option<&'a [Dimension]>,
    /// Active subscripts for A2A context (element names for each dimension)
    active_subscripts: Option<&'a [CanonicalElementName]>,
}

impl<'a> Pass1Context<'a> {
    /// Create a new pass 1 context without A2A information.
    /// Dimension and DimPosition references will block decomposition.
    pub fn new() -> Self {
        Self {
            next_temp_id: 0,
            temp_assignments: Vec::new(),
            active_dimensions: None,
            active_subscripts: None,
        }
    }

    /// Create a new pass 1 context with A2A information.
    /// Dimension and DimPosition references will be resolved to concrete indices.
    pub fn with_a2a_context(
        active_dimensions: &'a [Dimension],
        active_subscripts: &'a [CanonicalElementName],
    ) -> Self {
        Self {
            next_temp_id: 0,
            temp_assignments: Vec::new(),
            active_dimensions: Some(active_dimensions),
            active_subscripts: Some(active_subscripts),
        }
    }

    /// Allocate a new temp array ID
    fn allocate_temp_id(&mut self) -> u32 {
        let id = self.next_temp_id;
        self.next_temp_id += 1;
        id
    }

    /// Take the accumulated temp assignments
    pub fn take_assignments(&mut self) -> Vec<Expr3> {
        std::mem::take(&mut self.temp_assignments)
    }

    /// Apply pass 1 transformation to an expression.
    /// Returns the transformed expression and accumulates any AssignTemp nodes.
    pub fn transform(&mut self, expr: Expr3) -> Expr3 {
        self.transform_inner(expr).0
    }

    /// Internal transform that returns (transformed_expr, has_a2a_dimension_ref).
    /// The bool tracks whether the expression contains A2A dimension references,
    /// computed during the single recursive pass to avoid O(n²) tree walks.
    fn transform_inner(&mut self, expr: Expr3) -> (Expr3, bool) {
        match expr {
            // Constants and simple vars don't need transformation and have no A2A refs
            Expr3::Const(_, _, _) | Expr3::Var(_, _, _) => (expr, false),

            // Subscripts - check indices for A2A refs (Dimension, DimPosition)
            Expr3::Subscript(id, indices, bounds, loc) => {
                let mut has_a2a = false;
                let new_indices: Vec<_> = indices
                    .into_iter()
                    .map(|idx| {
                        let (new_idx, idx_has_a2a) = self.transform_index_expr_inner(idx);
                        has_a2a = has_a2a || idx_has_a2a;
                        new_idx
                    })
                    .collect();
                (Expr3::Subscript(id, new_indices, bounds, loc), has_a2a)
            }

            // Builtins - check if decomposition is needed for array builtins
            Expr3::App(builtin, bounds, loc) => {
                let (transformed_builtin, has_a2a) = self.transform_builtin_inner(builtin);
                (Expr3::App(transformed_builtin, bounds, loc), has_a2a)
            }

            // Binary ops - recurse into operands
            Expr3::Op2(op, left, right, bounds, loc) => {
                let (new_left, left_a2a) = self.transform_inner(*left);
                let (new_right, right_a2a) = self.transform_inner(*right);
                (
                    Expr3::Op2(op, Box::new(new_left), Box::new(new_right), bounds, loc),
                    left_a2a || right_a2a,
                )
            }

            // Unary ops - recurse into operand
            Expr3::Op1(op, inner, bounds, loc) => {
                let (new_inner, has_a2a) = self.transform_inner(*inner);
                (Expr3::Op1(op, Box::new(new_inner), bounds, loc), has_a2a)
            }

            // If expressions - recurse into all branches
            Expr3::If(cond, then_expr, else_expr, bounds, loc) => {
                let (new_cond, cond_a2a) = self.transform_inner(*cond);
                let (new_then, then_a2a) = self.transform_inner(*then_expr);
                let (new_else, else_a2a) = self.transform_inner(*else_expr);
                (
                    Expr3::If(
                        Box::new(new_cond),
                        Box::new(new_then),
                        Box::new(new_else),
                        bounds,
                        loc,
                    ),
                    cond_a2a || then_a2a || else_a2a,
                )
            }

            // Already-processed array variants - pass through, no A2A refs
            Expr3::StaticSubscript(_, _, _, _)
            | Expr3::TempArray(_, _, _)
            | Expr3::TempArrayElement(_, _, _, _) => (expr, false),

            // AssignTemp - recurse into the expression being assigned
            Expr3::AssignTemp(id, inner, view) => {
                let (new_inner, has_a2a) = self.transform_inner(*inner);
                (Expr3::AssignTemp(id, Box::new(new_inner), view), has_a2a)
            }
        }
    }

    /// Transform an index expression, returning (result, has_a2a_ref).
    ///
    /// When A2A context is available, Dimension and DimPosition references
    /// are resolved to concrete indices, allowing decomposition to proceed.
    fn transform_index_expr_inner(&mut self, idx: IndexExpr3) -> (IndexExpr3, bool) {
        match idx {
            IndexExpr3::Range(start, end, loc) => {
                let (new_start, start_a2a) = self.transform_inner(start);
                let (new_end, end_a2a) = self.transform_inner(end);
                (
                    IndexExpr3::Range(new_start, new_end, loc),
                    start_a2a || end_a2a,
                )
            }
            IndexExpr3::Expr(e) => {
                let (new_e, has_a2a) = self.transform_inner(e);
                (IndexExpr3::Expr(new_e), has_a2a)
            }
            // Dimension reference - try to resolve if we have A2A context
            IndexExpr3::Dimension(dim_name, loc) => {
                if let Some(active_dims) = self.active_dimensions
                    && let Some(active_subs) = self.active_subscripts
                {
                    // Find the active dimension that matches this dimension name
                    for (dim, sub) in active_dims.iter().zip(active_subs.iter()) {
                        let active_dim_name = CanonicalDimensionName::from_raw(dim.name());
                        if active_dim_name.as_str() == dim_name.as_str() {
                            // Found a match - resolve to the concrete index
                            if let Some(index) = Self::subscript_to_index(dim, sub) {
                                let const_expr = Expr3::Const(index.to_string(), index, loc);
                                return (IndexExpr3::Expr(const_expr), false);
                            }
                            // subscript_to_index returned None - subscript not valid for dimension.
                            // This is a bug in the caller since active_subscripts should always
                            // be valid elements. Leave the dimension unresolved so we fail later
                            // with a more informative error instead of silently using index 1.
                        }
                    }
                }
                // No A2A context or dimension not found - leave as unresolved
                (IndexExpr3::Dimension(dim_name, loc), true)
            }
            // DimPosition (@1, @2, etc.) - these are dimension bindings, NOT simple lookups.
            // @N means "use the current iteration value of output dimension N" and creates
            // a mapping between input and output dimensions during iteration.
            // This is fundamentally different from resolving to a constant - it must be
            // preserved for the view builder to handle dimension binding correctly.
            // Therefore, DimPosition is always treated as an A2A reference that blocks
            // decomposition in pass 1.
            IndexExpr3::DimPosition(pos, loc) => (IndexExpr3::DimPosition(pos, loc), true),
            // StarRange doesn't indicate A2A ref by itself
            IndexExpr3::StarRange(_, _) => (idx, false),
        }
    }

    /// Convert a dimension + subscript to its 1-based index value.
    /// Uses Dimension::get_offset to find the 0-based offset, then adds 1.
    /// Returns None if the subscript is not a valid element of the dimension.
    fn subscript_to_index(dim: &Dimension, subscript: &CanonicalElementName) -> Option<f64> {
        dim.get_offset(subscript).map(|offset| (offset + 1) as f64)
    }

    /// Transform a builtin function, returning (result, has_a2a_ref).
    fn transform_builtin_inner(&mut self, builtin: BuiltinFn<Expr3>) -> (BuiltinFn<Expr3>, bool) {
        use crate::builtins::BuiltinFn::*;

        match builtin {
            // Array reduction builtins - may need decomposition
            Sum(arg) => {
                let (new_arg, has_a2a) = self.maybe_decompose_array_arg_inner(*arg);
                (Sum(Box::new(new_arg)), has_a2a)
            }
            Mean(args) => {
                let mut has_a2a = false;
                let new_args: Vec<_> = args
                    .into_iter()
                    .map(|e| {
                        let (new_e, e_has_a2a) = self.maybe_decompose_array_arg_inner(e);
                        has_a2a = has_a2a || e_has_a2a;
                        new_e
                    })
                    .collect();
                (Mean(new_args), has_a2a)
            }
            Stddev(arg) => {
                let (new_arg, has_a2a) = self.maybe_decompose_array_arg_inner(*arg);
                (Stddev(Box::new(new_arg)), has_a2a)
            }
            Size(arg) => {
                let (new_arg, has_a2a) = self.maybe_decompose_array_arg_inner(*arg);
                (Size(Box::new(new_arg)), has_a2a)
            }
            // Min/Max with single arg are array reductions
            Min(arg, None) => {
                let (new_arg, has_a2a) = self.maybe_decompose_array_arg_inner(*arg);
                (Min(Box::new(new_arg), None), has_a2a)
            }
            Max(arg, None) => {
                let (new_arg, has_a2a) = self.maybe_decompose_array_arg_inner(*arg);
                (Max(Box::new(new_arg), None), has_a2a)
            }

            // Two-arg Min/Max are scalar operations - just recurse
            Min(a, Some(b)) => {
                let (new_a, a_has_a2a) = self.transform_inner(*a);
                let (new_b, b_has_a2a) = self.transform_inner(*b);
                (
                    Min(Box::new(new_a), Some(Box::new(new_b))),
                    a_has_a2a || b_has_a2a,
                )
            }
            Max(a, Some(b)) => {
                let (new_a, a_has_a2a) = self.transform_inner(*a);
                let (new_b, b_has_a2a) = self.transform_inner(*b);
                (
                    Max(Box::new(new_a), Some(Box::new(new_b))),
                    a_has_a2a || b_has_a2a,
                )
            }

            // Other builtins - recurse into arguments
            Lookup(id, e, loc) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Lookup(id, Box::new(new_e), loc), has_a2a)
            }
            Abs(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Abs(Box::new(new_e)), has_a2a)
            }
            Arccos(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Arccos(Box::new(new_e)), has_a2a)
            }
            Arcsin(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Arcsin(Box::new(new_e)), has_a2a)
            }
            Arctan(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Arctan(Box::new(new_e)), has_a2a)
            }
            Cos(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Cos(Box::new(new_e)), has_a2a)
            }
            Exp(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Exp(Box::new(new_e)), has_a2a)
            }
            Int(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Int(Box::new(new_e)), has_a2a)
            }
            Ln(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Ln(Box::new(new_e)), has_a2a)
            }
            Log10(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Log10(Box::new(new_e)), has_a2a)
            }
            Sign(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Sign(Box::new(new_e)), has_a2a)
            }
            Sin(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Sin(Box::new(new_e)), has_a2a)
            }
            Sqrt(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Sqrt(Box::new(new_e)), has_a2a)
            }
            Tan(e) => {
                let (new_e, has_a2a) = self.transform_inner(*e);
                (Tan(Box::new(new_e)), has_a2a)
            }
            Step(a, b) => {
                let (new_a, a_has_a2a) = self.transform_inner(*a);
                let (new_b, b_has_a2a) = self.transform_inner(*b);
                (
                    Step(Box::new(new_a), Box::new(new_b)),
                    a_has_a2a || b_has_a2a,
                )
            }
            Pulse(a, b, c) => {
                let (new_a, a_has_a2a) = self.transform_inner(*a);
                let (new_b, b_has_a2a) = self.transform_inner(*b);
                let (new_c, c_has_a2a) = match c {
                    Some(e) => {
                        let (new_e, has_a2a) = self.transform_inner(*e);
                        (Some(Box::new(new_e)), has_a2a)
                    }
                    None => (None, false),
                };
                (
                    Pulse(Box::new(new_a), Box::new(new_b), new_c),
                    a_has_a2a || b_has_a2a || c_has_a2a,
                )
            }
            Ramp(a, b, c) => {
                let (new_a, a_has_a2a) = self.transform_inner(*a);
                let (new_b, b_has_a2a) = self.transform_inner(*b);
                let (new_c, c_has_a2a) = match c {
                    Some(e) => {
                        let (new_e, has_a2a) = self.transform_inner(*e);
                        (Some(Box::new(new_e)), has_a2a)
                    }
                    None => (None, false),
                };
                (
                    Ramp(Box::new(new_a), Box::new(new_b), new_c),
                    a_has_a2a || b_has_a2a || c_has_a2a,
                )
            }
            SafeDiv(a, b, c) => {
                let (new_a, a_has_a2a) = self.transform_inner(*a);
                let (new_b, b_has_a2a) = self.transform_inner(*b);
                let (new_c, c_has_a2a) = match c {
                    Some(e) => {
                        let (new_e, has_a2a) = self.transform_inner(*e);
                        (Some(Box::new(new_e)), has_a2a)
                    }
                    None => (None, false),
                };
                (
                    SafeDiv(Box::new(new_a), Box::new(new_b), new_c),
                    a_has_a2a || b_has_a2a || c_has_a2a,
                )
            }
            Rank(e, opt) => {
                let (new_e, e_has_a2a) = self.transform_inner(*e);
                let (new_opt, opt_has_a2a) = match opt {
                    Some((a, b)) => {
                        let (new_a, a_has_a2a) = self.transform_inner(*a);
                        let (new_b, b_has_a2a) = match b {
                            Some(e) => {
                                let (new_e, has_a2a) = self.transform_inner(*e);
                                (Some(Box::new(new_e)), has_a2a)
                            }
                            None => (None, false),
                        };
                        (Some((Box::new(new_a), new_b)), a_has_a2a || b_has_a2a)
                    }
                    None => (None, false),
                };
                (Rank(Box::new(new_e), new_opt), e_has_a2a || opt_has_a2a)
            }

            // 0-arity builtins - no A2A refs
            Inf | Pi | Time | TimeStep | StartTime | FinalTime | IsModuleInput(_, _) => {
                (builtin, false)
            }
        }
    }

    /// Check if an array argument needs decomposition and decompose if so.
    /// Returns (transformed_expr, has_a2a_ref).
    ///
    /// Decomposition is needed when:
    /// 1. The expression is complex (Op1, Op2, If) - not a simple Subscript/Var/TempArray
    /// 2. The expression does NOT contain A2A dimension references (defer those to pass 2)
    /// 3. The expression has array bounds (produces an array result)
    fn maybe_decompose_array_arg_inner(&mut self, arg: Expr3) -> (Expr3, bool) {
        // First, recursively transform the argument (handles nested cases)
        // This also computes has_a2a in O(n) during the single pass
        let (transformed, has_a2a) = self.transform_inner(arg);

        // Check if this needs decomposition
        if !Self::needs_decomposition(&transformed) {
            return (transformed, has_a2a);
        }

        // If this contains A2A dimension references, defer to pass 2
        if has_a2a {
            return (transformed, has_a2a);
        }

        // Get the array dimensions from the expression bounds
        let dims = match transformed.get_array_bounds() {
            Some(bounds) => bounds.dims().to_vec(),
            None => {
                // No array bounds - might be a scalar or already decomposed
                // Check for array view
                if let Some(view) = transformed.get_array_view() {
                    view.dims.clone()
                } else {
                    // Scalar expression - no decomposition needed
                    return (transformed, has_a2a);
                }
            }
        };

        // Edge case: empty dimensions means 0-dimensional array (scalar-like)
        // No decomposition needed for this case
        if dims.is_empty() {
            return (transformed, has_a2a);
        }

        // Create the view for the temp array
        let view = ArrayView::contiguous(dims);

        // Allocate a temp ID and create the decomposition
        let temp_id = self.allocate_temp_id();
        let loc = transformed.get_loc();

        // Add the AssignTemp to our accumulated assignments
        let assign = Expr3::AssignTemp(temp_id, Box::new(transformed), view.clone());
        self.temp_assignments.push(assign);

        // Return a TempArray reference (no A2A refs since we only decompose non-A2A exprs)
        (Expr3::TempArray(temp_id, view, loc), false)
    }

    /// Check if an expression needs to be decomposed into a temp array.
    /// Returns true for complex expressions (Op1, Op2, If) that produce arrays.
    fn needs_decomposition(expr: &Expr3) -> bool {
        match expr {
            // Complex expressions that might need decomposition
            Expr3::Op1(_, _, bounds, _)
            | Expr3::Op2(_, _, _, bounds, _)
            | Expr3::If(_, _, _, bounds, _) => {
                // Only decompose if it produces an array
                bounds.is_some()
            }
            // Simple expressions don't need decomposition
            Expr3::Const(_, _, _)
            | Expr3::Var(_, _, _)
            | Expr3::Subscript(_, _, _, _)
            | Expr3::StaticSubscript(_, _, _, _)
            | Expr3::TempArray(_, _, _)
            | Expr3::TempArrayElement(_, _, _, _) => false,
            // App might need decomposition if it contains complex args
            // but the result of the app itself doesn't need further decomposition
            Expr3::App(_, _, _) => false,
            // Already an assignment - don't re-decompose
            Expr3::AssignTemp(_, _, _) => false,
        }
    }
}

impl Default for Pass1Context<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Expr3 {
    pub fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr3::Const(s, n, _) => Expr3::Const(s, n, loc),
            Expr3::Var(id, bounds, _) => Expr3::Var(id, bounds, loc),
            Expr3::App(builtin, bounds, _) => {
                let builtin = builtin.map(|e| e.strip_loc());
                Expr3::App(builtin, bounds, loc)
            }
            Expr3::Subscript(id, args, bounds, _) => {
                let args = args.into_iter().map(|a| a.strip_loc()).collect();
                Expr3::Subscript(id, args, bounds, loc)
            }
            Expr3::Op1(op, inner, bounds, _) => {
                Expr3::Op1(op, Box::new(inner.strip_loc()), bounds, loc)
            }
            Expr3::Op2(op, l, r, bounds, _) => Expr3::Op2(
                op,
                Box::new(l.strip_loc()),
                Box::new(r.strip_loc()),
                bounds,
                loc,
            ),
            Expr3::If(c, t, f, bounds, _) => Expr3::If(
                Box::new(c.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                bounds,
                loc,
            ),
            Expr3::StaticSubscript(id, view, off, _) => Expr3::StaticSubscript(id, view, off, loc),
            Expr3::TempArray(id, view, _) => Expr3::TempArray(id, view, loc),
            Expr3::TempArrayElement(id, view, idx, _) => {
                Expr3::TempArrayElement(id, view, idx, loc)
            }
            Expr3::AssignTemp(id, expr, view) => {
                Expr3::AssignTemp(id, Box::new(expr.strip_loc()), view)
            }
        }
    }
}

#[cfg(test)]
impl IndexExpr3 {
    pub fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            IndexExpr3::StarRange(name, _) => IndexExpr3::StarRange(name, loc),
            IndexExpr3::Range(l, r, _) => IndexExpr3::Range(l.strip_loc(), r.strip_loc(), loc),
            IndexExpr3::DimPosition(n, _) => IndexExpr3::DimPosition(n, loc),
            IndexExpr3::Expr(e) => IndexExpr3::Expr(e.strip_loc()),
            IndexExpr3::Dimension(name, _) => IndexExpr3::Dimension(name, loc),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::canonicalize;

    #[test]
    fn test_expr3_const() {
        let expr = Expr3::Const("42".to_string(), 42.0, Loc::new(0, 2));
        assert_eq!(expr.get_loc(), Loc::new(0, 2));
        assert!(expr.get_array_bounds().is_none());
        assert!(expr.get_array_view().is_none());
    }

    #[test]
    fn test_expr3_var_scalar() {
        let expr = Expr3::Var(canonicalize("x"), None, Loc::new(0, 1));
        assert_eq!(expr.get_loc(), Loc::new(0, 1));
        assert!(expr.get_array_bounds().is_none());
    }

    #[test]
    fn test_expr3_var_array() {
        let bounds = ArrayBounds::Named {
            name: "arr".to_string(),
            dims: vec![3, 4],
            dim_names: None,
        };
        let expr = Expr3::Var(canonicalize("arr"), Some(bounds), Loc::new(0, 3));
        assert!(expr.get_array_bounds().is_some());
        assert_eq!(expr.get_array_bounds().unwrap().dims(), &[3, 4]);
    }

    #[test]
    fn test_expr3_static_subscript() {
        let view = ArrayView::contiguous(vec![3, 4]);
        let expr =
            Expr3::StaticSubscript(canonicalize("matrix"), view.clone(), 100, Loc::new(0, 6));

        assert_eq!(expr.get_loc(), Loc::new(0, 6));
        assert!(expr.get_array_bounds().is_none());
        assert!(expr.get_array_view().is_some());
        assert_eq!(expr.get_array_view().unwrap().dims, vec![3, 4]);

        if let Expr3::StaticSubscript(id, _, offset, _) = &expr {
            assert_eq!(id.as_str(), "matrix");
            assert_eq!(*offset, 100);
        }
    }

    #[test]
    fn test_expr3_temp_array() {
        let view = ArrayView::contiguous(vec![5]);
        let expr = Expr3::TempArray(7, view.clone(), Loc::new(0, 4));

        assert!(expr.get_array_view().is_some());
        if let Expr3::TempArray(id, v, _) = &expr {
            assert_eq!(*id, 7);
            assert_eq!(v.dims, vec![5]);
        }
    }

    #[test]
    fn test_expr3_assign_temp() {
        let inner = Expr3::Const("1".to_string(), 1.0, Loc::new(0, 1));
        let view = ArrayView::contiguous(vec![2, 3]);
        let expr = Expr3::AssignTemp(0, Box::new(inner), view);

        assert_eq!(expr.get_loc(), Loc::default());
        assert!(expr.get_array_view().is_some());
    }

    #[test]
    fn test_expr3_strip_loc() {
        let expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(0, 1))),
            Box::new(Expr3::Const("2".to_string(), 2.0, Loc::new(4, 5))),
            None,
            Loc::new(0, 5),
        );

        let stripped = expr.strip_loc();
        assert_eq!(stripped.get_loc(), Loc::default());

        if let Expr3::Op2(_, l, r, _, _) = stripped {
            assert_eq!(l.get_loc(), Loc::default());
            assert_eq!(r.get_loc(), Loc::default());
        }
    }

    #[test]
    fn test_index_expr3_get_loc() {
        assert_eq!(
            IndexExpr3::StarRange(CanonicalDimensionName::from_raw("dim"), Loc::new(1, 2))
                .get_loc(),
            Loc::new(1, 2)
        );
        assert_eq!(
            IndexExpr3::DimPosition(1, Loc::new(3, 4)).get_loc(),
            Loc::new(3, 4)
        );
    }

    // ========================================================================
    // Expr2 → Expr3 Lowering Tests
    // ========================================================================

    use std::collections::HashMap;

    /// Helper function to create indexed dimensions for testing
    fn indexed_dims(sizes: &[u32]) -> Vec<Dimension> {
        sizes
            .iter()
            .enumerate()
            .map(|(i, &size)| {
                Dimension::Indexed(CanonicalDimensionName::from_raw(&format!("dim{i}")), size)
            })
            .collect()
    }

    /// Helper function to create named dimensions for testing
    fn named_dim(name: &str, elements: &[&str]) -> Dimension {
        use crate::common::CanonicalElementName;
        use crate::dimensions::NamedDimension;

        let canonical_elements: Vec<CanonicalElementName> = elements
            .iter()
            .map(|e| CanonicalElementName::from_raw(e))
            .collect();

        let indexed_elements: HashMap<CanonicalElementName, usize> = canonical_elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.clone(), i))
            .collect();

        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                elements: canonical_elements,
                indexed_elements,
            },
        )
    }

    /// Test context for Expr3 lowering
    struct TestLowerContext {
        dimensions: HashMap<String, Vec<Dimension>>,
        dimension_names: std::collections::HashSet<String>,
    }

    impl TestLowerContext {
        fn new() -> Self {
            Self {
                dimensions: HashMap::new(),
                dimension_names: std::collections::HashSet::new(),
            }
        }

        fn with_var(mut self, name: &str, dims: Vec<Dimension>) -> Self {
            // Register dimension names from the dimensions
            for dim in &dims {
                self.dimension_names.insert(dim.name().to_lowercase());
            }
            self.dimensions.insert(name.to_string(), dims);
            self
        }

        fn with_dimension_name(mut self, name: &str) -> Self {
            self.dimension_names.insert(name.to_lowercase());
            self
        }
    }

    impl Expr3LowerContext for TestLowerContext {
        fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>> {
            self.dimensions.get(ident).cloned()
        }

        fn is_dimension_name(&self, ident: &str) -> bool {
            self.dimension_names.contains(&ident.to_lowercase())
        }
    }

    #[test]
    fn test_lower_scalar_var() {
        let ctx = TestLowerContext::new();
        let expr2 = Expr2::Var(canonicalize("x"), None, Loc::new(0, 1));

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Var(id, bounds, loc) => {
                assert_eq!(id.as_str(), "x");
                assert!(bounds.is_none());
                assert_eq!(loc, Loc::new(0, 1));
            }
            _ => panic!("Expected Var"),
        }
    }

    #[test]
    fn test_lower_bare_array_var_adds_subscripts() {
        // Test that a bare array variable gets implicit subscripts added
        let ctx = TestLowerContext::new().with_var("arr", indexed_dims(&[3, 4]));

        let bounds = ArrayBounds::Named {
            name: "arr".to_string(),
            dims: vec![3, 4],
            dim_names: Some(vec!["dim0".to_string(), "dim1".to_string()]),
        };
        let expr2 = Expr2::Var(canonicalize("arr"), Some(bounds), Loc::new(0, 3));

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(id, args, _, _) => {
                assert_eq!(id.as_str(), "arr");
                assert_eq!(args.len(), 2);

                // Both subscripts should be StarRange with the dimension names
                match &args[0] {
                    IndexExpr3::StarRange(name, _) => {
                        assert_eq!(name.as_str(), "dim0");
                    }
                    _ => panic!("Expected StarRange for first subscript"),
                }
                match &args[1] {
                    IndexExpr3::StarRange(name, _) => {
                        assert_eq!(name.as_str(), "dim1");
                    }
                    _ => panic!("Expected StarRange for second subscript"),
                }
            }
            _ => panic!("Expected Subscript, got {:?}", expr3),
        }
    }

    #[test]
    fn test_lower_wildcard_to_star_range() {
        // Test that arr[*] gets the wildcard resolved to the dimension name
        let ctx = TestLowerContext::new().with_var("vec", indexed_dims(&[5]));

        let expr2 = Expr2::Subscript(
            canonicalize("vec"),
            vec![IndexExpr2::Wildcard(Loc::new(4, 5))],
            None,
            Loc::new(0, 6),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(id, args, _, _) => {
                assert_eq!(id.as_str(), "vec");
                assert_eq!(args.len(), 1);

                match &args[0] {
                    IndexExpr3::StarRange(name, loc) => {
                        assert_eq!(name.as_str(), "dim0");
                        assert_eq!(*loc, Loc::new(4, 5)); // Preserves original wildcard location
                    }
                    _ => panic!("Expected StarRange"),
                }
            }
            _ => panic!("Expected Subscript"),
        }
    }

    #[test]
    fn test_lower_explicit_star_range_unchanged() {
        // Test that explicit *:SubDim is passed through unchanged
        let ctx = TestLowerContext::new().with_var("arr", indexed_dims(&[5]));

        let subdim_name = CanonicalDimensionName::from_raw("SubDim");
        let expr2 = Expr2::Subscript(
            canonicalize("arr"),
            vec![IndexExpr2::StarRange(subdim_name.clone(), Loc::new(4, 10))],
            None,
            Loc::new(0, 11),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(_, args, _, _) => {
                match &args[0] {
                    IndexExpr3::StarRange(name, _) => {
                        // Should preserve the user-specified subdimension name, not change it
                        assert_eq!(name.as_str(), "subdim");
                    }
                    _ => panic!("Expected StarRange"),
                }
            }
            _ => panic!("Expected Subscript"),
        }
    }

    #[test]
    fn test_lower_wildcard_on_scalar_errors() {
        // Test that using wildcard on a scalar variable produces an error
        let ctx = TestLowerContext::new(); // No dimensions for "scalar"

        let expr2 = Expr2::Subscript(
            canonicalize("scalar"),
            vec![IndexExpr2::Wildcard(Loc::new(7, 8))],
            None,
            Loc::new(0, 9),
        );

        let result = Expr3::from_expr2(&expr2, &ctx);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert_eq!(err.code, crate::common::ErrorCode::CantSubscriptScalar);
        assert_eq!(err.start, 7);
        assert_eq!(err.end, 8);
    }

    #[test]
    fn test_lower_mixed_subscripts() {
        // Test arr[*, 2] - wildcard and constant subscript
        let ctx = TestLowerContext::new().with_var("matrix", indexed_dims(&[3, 4]));

        let expr2 = Expr2::Subscript(
            canonicalize("matrix"),
            vec![
                IndexExpr2::Wildcard(Loc::new(7, 8)),
                IndexExpr2::Expr(Expr2::Const("2".to_string(), 2.0, Loc::new(10, 11))),
            ],
            None,
            Loc::new(0, 12),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(_, args, _, _) => {
                assert_eq!(args.len(), 2);

                // First subscript: wildcard → StarRange
                match &args[0] {
                    IndexExpr3::StarRange(name, _) => {
                        assert_eq!(name.as_str(), "dim0");
                    }
                    _ => panic!("Expected StarRange for first subscript"),
                }

                // Second subscript: constant expression
                match &args[1] {
                    IndexExpr3::Expr(Expr3::Const(_, val, _)) => {
                        assert_eq!(*val, 2.0);
                    }
                    _ => panic!("Expected Expr(Const) for second subscript"),
                }
            }
            _ => panic!("Expected Subscript"),
        }
    }

    #[test]
    fn test_lower_nested_expression() {
        // Test that lowering works recursively for nested expressions
        let ctx = TestLowerContext::new()
            .with_var("arr1", indexed_dims(&[3]))
            .with_var("arr2", indexed_dims(&[3]));

        // arr1 + arr2 (both bare arrays)
        let bounds1 = ArrayBounds::Named {
            name: "arr1".to_string(),
            dims: vec![3],
            dim_names: Some(vec!["dim0".to_string()]),
        };
        let bounds2 = ArrayBounds::Named {
            name: "arr2".to_string(),
            dims: vec![3],
            dim_names: Some(vec!["dim0".to_string()]),
        };

        let expr2 = Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Var(
                canonicalize("arr1"),
                Some(bounds1),
                Loc::new(0, 4),
            )),
            Box::new(Expr2::Var(
                canonicalize("arr2"),
                Some(bounds2),
                Loc::new(7, 11),
            )),
            None,
            Loc::new(0, 11),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        // Both arr1 and arr2 should be converted to Subscript with StarRange
        match expr3 {
            Expr3::Op2(BinaryOp::Add, left, right, _, _) => {
                match left.as_ref() {
                    Expr3::Subscript(id, args, _, _) => {
                        assert_eq!(id.as_str(), "arr1");
                        assert_eq!(args.len(), 1);
                        assert!(matches!(&args[0], IndexExpr3::StarRange(_, _)));
                    }
                    _ => panic!("Expected Subscript for left operand"),
                }
                match right.as_ref() {
                    Expr3::Subscript(id, args, _, _) => {
                        assert_eq!(id.as_str(), "arr2");
                        assert_eq!(args.len(), 1);
                        assert!(matches!(&args[0], IndexExpr3::StarRange(_, _)));
                    }
                    _ => panic!("Expected Subscript for right operand"),
                }
            }
            _ => panic!("Expected Op2"),
        }
    }

    #[test]
    fn test_lower_named_dimension() {
        // Test with named dimension (Cities with Boston, NYC, LA)
        let cities = named_dim("Cities", &["Boston", "NYC", "LA"]);
        let ctx = TestLowerContext::new().with_var("sales", vec![cities]);

        let expr2 = Expr2::Subscript(
            canonicalize("sales"),
            vec![IndexExpr2::Wildcard(Loc::new(6, 7))],
            None,
            Loc::new(0, 8),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(_, args, _, _) => match &args[0] {
                IndexExpr3::StarRange(name, _) => {
                    assert_eq!(name.as_str(), "cities");
                }
                _ => panic!("Expected StarRange"),
            },
            _ => panic!("Expected Subscript"),
        }
    }

    #[test]
    fn test_lower_multidimensional_wildcards() {
        // Test cube[*, *, 5] - 3D array with first two wildcards and third constant
        let ctx = TestLowerContext::new().with_var("cube", indexed_dims(&[3, 4, 5]));

        let expr2 = Expr2::Subscript(
            canonicalize("cube"),
            vec![
                IndexExpr2::Wildcard(Loc::new(5, 6)),
                IndexExpr2::Wildcard(Loc::new(8, 9)),
                IndexExpr2::Expr(Expr2::Const("5".to_string(), 5.0, Loc::new(11, 12))),
            ],
            None,
            Loc::new(0, 13),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(id, args, _, _) => {
                assert_eq!(id.as_str(), "cube");
                assert_eq!(args.len(), 3);

                // First two subscripts: wildcards → StarRange
                match &args[0] {
                    IndexExpr3::StarRange(name, _) => assert_eq!(name.as_str(), "dim0"),
                    _ => panic!("Expected StarRange for first subscript"),
                }
                match &args[1] {
                    IndexExpr3::StarRange(name, _) => assert_eq!(name.as_str(), "dim1"),
                    _ => panic!("Expected StarRange for second subscript"),
                }

                // Third subscript: constant expression
                match &args[2] {
                    IndexExpr3::Expr(Expr3::Const(_, val, _)) => assert_eq!(*val, 5.0),
                    _ => panic!("Expected Expr(Const) for third subscript"),
                }
            }
            _ => panic!("Expected Subscript"),
        }
    }

    #[test]
    fn test_lower_too_many_subscripts_errors() {
        // Test arr[*, *, *] on a 2D array - should error with MismatchedDimensions
        let ctx = TestLowerContext::new().with_var("matrix", indexed_dims(&[3, 4]));

        let expr2 = Expr2::Subscript(
            canonicalize("matrix"),
            vec![
                IndexExpr2::Wildcard(Loc::new(7, 8)),
                IndexExpr2::Wildcard(Loc::new(10, 11)),
                IndexExpr2::Wildcard(Loc::new(13, 14)), // This is out of bounds
            ],
            None,
            Loc::new(0, 15),
        );

        let result = Expr3::from_expr2(&expr2, &ctx);
        assert!(result.is_err());

        let err = result.unwrap_err();
        // Should be MismatchedDimensions, not CantSubscriptScalar
        assert_eq!(err.code, crate::common::ErrorCode::MismatchedDimensions);
        // Error location should point to the first out-of-bounds subscript
        assert_eq!(err.start, 13);
        assert_eq!(err.end, 14);
    }

    #[test]
    fn test_lower_dimension_name_subscript() {
        // Test that arr[DimName] where DimName is a dimension name
        // gets converted to IndexExpr3::Dimension
        let ctx = TestLowerContext::new()
            .with_var("arr", indexed_dims(&[5]))
            .with_dimension_name("MyDim");

        // arr[MyDim] - MyDim is a dimension name, not a variable
        let expr2 = Expr2::Subscript(
            canonicalize("arr"),
            vec![IndexExpr2::Expr(Expr2::Var(
                canonicalize("MyDim"),
                None,
                Loc::new(4, 9),
            ))],
            None,
            Loc::new(0, 10),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(id, args, _, _) => {
                assert_eq!(id.as_str(), "arr");
                assert_eq!(args.len(), 1);

                // The subscript should be converted to IndexExpr3::Dimension
                match &args[0] {
                    IndexExpr3::Dimension(name, loc) => {
                        assert_eq!(name.as_str(), "mydim");
                        assert_eq!(*loc, Loc::new(4, 9));
                    }
                    _ => panic!("Expected Dimension, got {:?}", args[0]),
                }
            }
            _ => panic!("Expected Subscript"),
        }
    }

    #[test]
    fn test_lower_non_dimension_var_subscript() {
        // Test that arr[x] where x is NOT a dimension name stays as Expr
        let ctx = TestLowerContext::new()
            .with_var("arr", indexed_dims(&[5]))
            .with_dimension_name("OtherDim"); // Not the one we're using

        // arr[x] - x is not a dimension name
        let expr2 = Expr2::Subscript(
            canonicalize("arr"),
            vec![IndexExpr2::Expr(Expr2::Var(
                canonicalize("x"),
                None,
                Loc::new(4, 5),
            ))],
            None,
            Loc::new(0, 6),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        match expr3 {
            Expr3::Subscript(_, args, _, _) => {
                // Should remain as Expr, not Dimension
                match &args[0] {
                    IndexExpr3::Expr(Expr3::Var(name, _, _)) => {
                        assert_eq!(name.as_str(), "x");
                    }
                    _ => panic!("Expected Expr(Var), got {:?}", args[0]),
                }
            }
            _ => panic!("Expected Subscript"),
        }
    }

    #[test]
    fn test_references_a2a_dimension() {
        let ctx = TestLowerContext::new()
            .with_var("arr", indexed_dims(&[3, 4]))
            .with_dimension_name("Row");

        // arr[*, Row] - has a dimension reference in second position
        let expr2 = Expr2::Subscript(
            canonicalize("arr"),
            vec![
                IndexExpr2::Wildcard(Loc::new(4, 5)),
                IndexExpr2::Expr(Expr2::Var(canonicalize("Row"), None, Loc::new(7, 10))),
            ],
            None,
            Loc::new(0, 11),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        // The expression should reference an A2A dimension
        assert!(expr3.references_a2a_dimension());

        // arr[*, *] - no dimension reference
        let expr2_no_dim = Expr2::Subscript(
            canonicalize("arr"),
            vec![
                IndexExpr2::Wildcard(Loc::new(4, 5)),
                IndexExpr2::Wildcard(Loc::new(7, 8)),
            ],
            None,
            Loc::new(0, 9),
        );

        let expr3_no_dim = Expr3::from_expr2(&expr2_no_dim, &ctx).unwrap();
        // Should not reference A2A dimension (wildcards are resolved to StarRange)
        assert!(!expr3_no_dim.references_a2a_dimension());
    }

    // =========================================================================
    // Pass 1 Tests
    // =========================================================================

    #[test]
    fn test_pass1_no_decomposition_simple_subscript() {
        // SUM(arr[*]) - simple subscript, no decomposition needed
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![5],
            dim_names: None,
        };

        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("dim"),
                Loc::new(4, 5),
            )],
            Some(arr_bounds),
            Loc::new(0, 6),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(subscript)), None, Loc::new(0, 10));

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(sum_expr.clone());
        let assignments = ctx.take_assignments();

        // No decomposition needed - no assignments generated
        assert!(assignments.is_empty());

        // Result should be structurally the same as input
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(matches!(*inner, Expr3::Subscript(_, _, _, _)));
            }
            _ => panic!("Expected App(Sum(...))"),
        }
    }

    #[test]
    fn test_pass1_decompose_sum_with_op2() {
        // SUM(arr[*] + 1) - should decompose
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![5],
            dim_names: None,
        };

        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("dim"),
                Loc::new(4, 5),
            )],
            Some(arr_bounds.clone()),
            Loc::new(0, 6),
        );

        let one = Expr3::Const("1".to_string(), 1.0, Loc::new(10, 11));

        // arr[*] + 1 - has array bounds because arr[*] is an array
        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(subscript),
            Box::new(one),
            Some(arr_bounds),
            Loc::new(0, 11),
        );

        let sum_expr = Expr3::App(
            BuiltinFn::Sum(Box::new(add_expr)),
            None, // SUM produces scalar
            Loc::new(0, 15),
        );

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // Should have one assignment
        assert_eq!(assignments.len(), 1);

        // Check the assignment
        match &assignments[0] {
            Expr3::AssignTemp(id, expr, view) => {
                assert_eq!(*id, 0);
                assert_eq!(view.dims, vec![5]);
                // The expression should be the Op2
                assert!(matches!(**expr, Expr3::Op2(BinaryOp::Add, _, _, _, _)));
            }
            _ => panic!("Expected AssignTemp"),
        }

        // Check the result uses TempArray
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => match *inner {
                Expr3::TempArray(id, view, _) => {
                    assert_eq!(id, 0);
                    assert_eq!(view.dims, vec![5]);
                }
                _ => panic!("Expected TempArray, got {:?}", inner),
            },
            _ => panic!("Expected App(Sum(...))"),
        }
    }

    #[test]
    fn test_pass1_skip_a2a_dimension_reference() {
        // SUM(arr[*, DimName]) - has A2A dimension reference, should NOT decompose
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3, 4],
            dim_names: None,
        };

        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![
                IndexExpr3::StarRange(CanonicalDimensionName::from_raw("row"), Loc::new(4, 5)),
                IndexExpr3::Dimension(CanonicalDimensionName::from_raw("col"), Loc::new(7, 10)),
            ],
            Some(arr_bounds.clone()),
            Loc::new(0, 11),
        );

        let one = Expr3::Const("1".to_string(), 1.0, Loc::new(14, 15));

        // arr[*, Col] + 1 - has dimension reference
        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(subscript),
            Box::new(one),
            Some(arr_bounds),
            Loc::new(0, 15),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(add_expr)), None, Loc::new(0, 20));

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // Should NOT decompose because of A2A dimension reference
        assert!(assignments.is_empty());

        // The Op2 should still be there (not replaced with TempArray)
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(matches!(*inner, Expr3::Op2(BinaryOp::Add, _, _, _, _)));
            }
            _ => panic!("Expected App(Sum(...))"),
        }
    }

    #[test]
    fn test_pass1_nested_sums() {
        // SUM(SUM(inner) + outer)
        // The inner SUM reduces to scalar, so no decomposition is triggered
        // for the outer addition (since one operand is scalar).
        let inner_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3],
            dim_names: None,
        };

        let inner_sub = Expr3::Subscript(
            canonicalize("inner"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("a"),
                Loc::new(0, 1),
            )],
            Some(inner_bounds),
            Loc::new(0, 5),
        );

        // SUM(inner[*]) - produces scalar
        let inner_sum = Expr3::App(BuiltinFn::Sum(Box::new(inner_sub)), None, Loc::new(0, 10));

        let outer_bounds = ArrayBounds::Temp {
            id: 1,
            dims: vec![5],
            dim_names: None,
        };

        let outer_sub = Expr3::Subscript(
            canonicalize("outer"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("b"),
                Loc::new(0, 1),
            )],
            Some(outer_bounds.clone()),
            Loc::new(0, 5),
        );

        // SUM(inner) + outer[*] - outer is array, but inner_sum is scalar
        // This still has array bounds from outer
        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(inner_sum),
            Box::new(outer_sub),
            Some(outer_bounds),
            Loc::new(0, 15),
        );

        // SUM(SUM(inner[*]) + outer[*])
        let outer_sum = Expr3::App(BuiltinFn::Sum(Box::new(add_expr)), None, Loc::new(0, 20));

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(outer_sum);
        let assignments = ctx.take_assignments();

        // Should decompose the addition into a temp
        assert_eq!(assignments.len(), 1);

        match &assignments[0] {
            Expr3::AssignTemp(_, _, view) => {
                assert_eq!(view.dims, vec![5]); // outer's dimensions
            }
            _ => panic!("Expected AssignTemp"),
        }

        // Result should reference the temp
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(matches!(*inner, Expr3::TempArray(_, _, _)));
            }
            _ => panic!("Expected App(Sum(TempArray))"),
        }
    }

    #[test]
    fn test_pass1_multiple_decompositions() {
        // SUM(a[*] + 1) + SUM(b[*] * 2) - two separate decompositions
        let a_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3],
            dim_names: None,
        };
        let b_bounds = ArrayBounds::Temp {
            id: 1,
            dims: vec![4],
            dim_names: None,
        };

        // a[*] + 1
        let a_sub = Expr3::Subscript(
            canonicalize("a"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("x"),
                Loc::new(0, 1),
            )],
            Some(a_bounds.clone()),
            Loc::new(0, 3),
        );
        let a_add = Expr3::Op2(
            BinaryOp::Add,
            Box::new(a_sub),
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(6, 7))),
            Some(a_bounds),
            Loc::new(0, 7),
        );

        // b[*] * 2
        let b_sub = Expr3::Subscript(
            canonicalize("b"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("y"),
                Loc::new(0, 1),
            )],
            Some(b_bounds.clone()),
            Loc::new(0, 3),
        );
        let b_mul = Expr3::Op2(
            BinaryOp::Mul,
            Box::new(b_sub),
            Box::new(Expr3::Const("2".to_string(), 2.0, Loc::new(6, 7))),
            Some(b_bounds),
            Loc::new(0, 7),
        );

        // SUM(a[*] + 1) - produces scalar
        let sum_a = Expr3::App(BuiltinFn::Sum(Box::new(a_add)), None, Loc::new(0, 12));

        // SUM(b[*] * 2) - produces scalar
        let sum_b = Expr3::App(BuiltinFn::Sum(Box::new(b_mul)), None, Loc::new(0, 12));

        // SUM(a) + SUM(b) - scalar + scalar
        let total = Expr3::Op2(
            BinaryOp::Add,
            Box::new(sum_a),
            Box::new(sum_b),
            None, // scalar result
            Loc::new(0, 25),
        );

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(total);
        let assignments = ctx.take_assignments();

        // Should have two decompositions
        assert_eq!(assignments.len(), 2);

        // First decomposition for a[*] + 1
        match &assignments[0] {
            Expr3::AssignTemp(id, _, view) => {
                assert_eq!(*id, 0);
                assert_eq!(view.dims, vec![3]);
            }
            _ => panic!("Expected AssignTemp"),
        }

        // Second decomposition for b[*] * 2
        match &assignments[1] {
            Expr3::AssignTemp(id, _, view) => {
                assert_eq!(*id, 1);
                assert_eq!(view.dims, vec![4]);
            }
            _ => panic!("Expected AssignTemp"),
        }

        // Result should be Op2(Add, App(Sum(TempArray)), App(Sum(TempArray)))
        match result {
            Expr3::Op2(BinaryOp::Add, left, right, None, _) => {
                assert!(matches!(
                    *left,
                    Expr3::App(BuiltinFn::Sum(ref inner), _, _) if matches!(**inner, Expr3::TempArray(0, _, _))
                ));
                assert!(matches!(
                    *right,
                    Expr3::App(BuiltinFn::Sum(ref inner), _, _) if matches!(**inner, Expr3::TempArray(1, _, _))
                ));
            }
            _ => panic!("Expected Op2(Add, App, App)"),
        }
    }

    #[test]
    fn test_pass1_if_with_a2a_branch() {
        // IF(cond, arr[*, DimName], arr[*] + 1)
        // First branch has A2A, second does NOT - should still NOT decompose
        // because we can't partially decompose an IF expression
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3, 4],
            dim_names: None,
        };

        let cond = Expr3::Var(canonicalize("cond"), None, Loc::new(0, 4));

        // true branch: arr[*, DimName] - has A2A reference
        let true_branch = Expr3::Subscript(
            canonicalize("arr"),
            vec![
                IndexExpr3::StarRange(CanonicalDimensionName::from_raw("row"), Loc::new(0, 1)),
                IndexExpr3::Dimension(CanonicalDimensionName::from_raw("col"), Loc::new(3, 6)),
            ],
            Some(arr_bounds.clone()),
            Loc::new(0, 7),
        );

        // false branch: arr2[*] + 1 - no A2A, could decompose
        let arr2_bounds = ArrayBounds::Temp {
            id: 1,
            dims: vec![5],
            dim_names: None,
        };
        let arr2_sub = Expr3::Subscript(
            canonicalize("arr2"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("x"),
                Loc::new(0, 1),
            )],
            Some(arr2_bounds.clone()),
            Loc::new(0, 4),
        );
        let false_branch = Expr3::Op2(
            BinaryOp::Add,
            Box::new(arr2_sub),
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(7, 8))),
            Some(arr2_bounds),
            Loc::new(0, 8),
        );

        // The IF expression has array bounds
        let if_expr = Expr3::If(
            Box::new(cond),
            Box::new(true_branch),
            Box::new(false_branch),
            Some(arr_bounds),
            Loc::new(0, 30),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(if_expr)), None, Loc::new(0, 35));

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // Should NOT decompose because the true branch has A2A reference
        assert!(assignments.is_empty());

        // Result should still have the IF expression
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(matches!(*inner, Expr3::If(_, _, _, _, _)));
            }
            _ => panic!("Expected App(Sum(If(...)))"),
        }
    }

    #[test]
    fn test_pass1_dim_position_blocks_decomposition() {
        // SUM(arr[@1]) - DimPosition is also an A2A reference, should NOT decompose
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![5],
            dim_names: None,
        };

        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![IndexExpr3::DimPosition(1, Loc::new(4, 6))],
            Some(arr_bounds.clone()),
            Loc::new(0, 7),
        );

        // arr[@1] + 1
        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(subscript),
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(10, 11))),
            Some(arr_bounds),
            Loc::new(0, 11),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(add_expr)), None, Loc::new(0, 15));

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // Should NOT decompose because DimPosition is an A2A reference
        assert!(
            assignments.is_empty(),
            "DimPosition should block decomposition"
        );

        // Result should still have the Op2 expression
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(matches!(*inner, Expr3::Op2(BinaryOp::Add, _, _, _, _)));
            }
            _ => panic!("Expected App(Sum(Op2(...)))"),
        }
    }

    #[test]
    fn test_pass1_deeply_nested_decomposition() {
        // SUM((arr[*] + 1) * (arr2[*] - 1))
        // The expression inside SUM is a complex non-trivial Op2 tree that should be decomposed
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3],
            dim_names: None,
        };
        let arr2_bounds = ArrayBounds::Temp {
            id: 1,
            dims: vec![3],
            dim_names: None,
        };

        let arr_sub = Expr3::Subscript(
            canonicalize("arr"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("x"),
                Loc::new(0, 1),
            )],
            Some(arr_bounds.clone()),
            Loc::new(0, 4),
        );

        let arr2_sub = Expr3::Subscript(
            canonicalize("arr2"),
            vec![IndexExpr3::StarRange(
                CanonicalDimensionName::from_raw("x"),
                Loc::new(0, 1),
            )],
            Some(arr2_bounds.clone()),
            Loc::new(0, 5),
        );

        // arr[*] + 1
        let left_op = Expr3::Op2(
            BinaryOp::Add,
            Box::new(arr_sub),
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(7, 8))),
            Some(arr_bounds.clone()),
            Loc::new(0, 8),
        );

        // arr2[*] - 1
        let right_op = Expr3::Op2(
            BinaryOp::Sub,
            Box::new(arr2_sub),
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(15, 16))),
            Some(arr2_bounds),
            Loc::new(0, 16),
        );

        // (arr[*] + 1) * (arr2[*] - 1) - the multiply has array bounds
        let mul_expr = Expr3::Op2(
            BinaryOp::Mul,
            Box::new(left_op),
            Box::new(right_op),
            Some(arr_bounds),
            Loc::new(0, 25),
        );

        // SUM((arr[*] + 1) * (arr2[*] - 1))
        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(mul_expr)), None, Loc::new(0, 30));

        let mut ctx = Pass1Context::new();
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // Should have one decomposition for the multiply expression
        assert_eq!(assignments.len(), 1);

        match &assignments[0] {
            Expr3::AssignTemp(id, expr, view) => {
                assert_eq!(*id, 0);
                assert_eq!(view.dims, vec![3]);
                // The expression should be the multiply
                assert!(matches!(**expr, Expr3::Op2(BinaryOp::Mul, _, _, _, _)));
            }
            _ => panic!("Expected AssignTemp"),
        }

        // Result should reference the temp
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(matches!(*inner, Expr3::TempArray(0, _, _)));
            }
            _ => panic!("Expected App(Sum(TempArray))"),
        }
    }

    // =========================================================================
    // Pass 1 with A2A Context Tests (Pass 2 behavior)
    // =========================================================================

    use crate::common::CanonicalElementName;

    #[test]
    fn test_pass1_with_a2a_context_resolves_dimension() {
        // SUM(arr[*, Col] + 1) - previously blocked, but with A2A context should decompose
        // because Col resolves to a concrete index
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3, 4],
            dim_names: Some(vec!["row".to_string(), "col".to_string()]),
        };

        // Create arr[*, Col] - has Dimension reference
        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![
                IndexExpr3::StarRange(CanonicalDimensionName::from_raw("row"), Loc::new(4, 5)),
                IndexExpr3::Dimension(CanonicalDimensionName::from_raw("col"), Loc::new(7, 10)),
            ],
            Some(arr_bounds.clone()),
            Loc::new(0, 11),
        );

        let one = Expr3::Const("1".to_string(), 1.0, Loc::new(14, 15));

        // arr[*, Col] + 1 - the array bounds after subscripting should be [3]
        // because Col is pinned to a single value
        let subscripted_bounds = ArrayBounds::Temp {
            id: 1,
            dims: vec![3], // Only Row dimension remains
            dim_names: Some(vec!["row".to_string()]),
        };
        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(subscript),
            Box::new(one),
            Some(subscripted_bounds),
            Loc::new(0, 15),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(add_expr)), None, Loc::new(0, 20));

        // Create A2A context: we're evaluating for Col = 2 (0-based index 1)
        let row_dim = Dimension::Indexed(CanonicalDimensionName::from_raw("row"), 3);
        let col_dim = Dimension::Indexed(CanonicalDimensionName::from_raw("col"), 4);
        let active_dimensions = vec![row_dim, col_dim];
        let active_subscripts = vec![
            CanonicalElementName::from_raw("1"), // Row = 1
            CanonicalElementName::from_raw("2"), // Col = 2
        ];

        let mut ctx = Pass1Context::with_a2a_context(&active_dimensions, &active_subscripts);
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // NOW should decompose because the Dimension ref is resolved
        assert_eq!(assignments.len(), 1, "Should decompose with A2A context");

        // Check the assignment has correct dims (only Row, since Col is pinned)
        match &assignments[0] {
            Expr3::AssignTemp(id, _, view) => {
                assert_eq!(id, &0);
                assert_eq!(view.dims, vec![3], "Should have Row dimension only");
            }
            _ => panic!("Expected AssignTemp"),
        }

        // Result should reference the temp
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(
                    matches!(*inner, Expr3::TempArray(0, _, _)),
                    "Expected TempArray, got {:?}",
                    inner
                );
            }
            _ => panic!("Expected App(Sum(TempArray))"),
        }
    }

    #[test]
    fn test_pass1_without_a2a_context_still_blocks() {
        // Same expression as above, but without A2A context
        // SUM(arr[*, Col] + 1) - should NOT decompose
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3, 4],
            dim_names: Some(vec!["row".to_string(), "col".to_string()]),
        };

        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![
                IndexExpr3::StarRange(CanonicalDimensionName::from_raw("row"), Loc::new(4, 5)),
                IndexExpr3::Dimension(CanonicalDimensionName::from_raw("col"), Loc::new(7, 10)),
            ],
            Some(arr_bounds.clone()),
            Loc::new(0, 11),
        );

        let one = Expr3::Const("1".to_string(), 1.0, Loc::new(14, 15));

        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(subscript),
            Box::new(one),
            Some(arr_bounds),
            Loc::new(0, 15),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(add_expr)), None, Loc::new(0, 20));

        // NO A2A context
        let mut ctx = Pass1Context::new();
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // Should NOT decompose
        assert!(
            assignments.is_empty(),
            "Should not decompose without A2A context"
        );

        // Result should still have the Op2 expression
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                assert!(matches!(*inner, Expr3::Op2(BinaryOp::Add, _, _, _, _)));
            }
            _ => panic!("Expected App(Sum(Op2(...)))"),
        }
    }

    #[test]
    fn test_pass1_dim_position_not_resolved_even_with_a2a() {
        // SUM(arr[@1] + 1) - DimPosition should NOT be resolved, even with A2A context.
        // DimPosition is a dimension binding mechanism, not a simple lookup.
        // It creates a mapping between input and output dimensions during iteration.
        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![5],
            dim_names: Some(vec!["x".to_string()]),
        };

        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![IndexExpr3::DimPosition(1, Loc::new(4, 6))],
            Some(arr_bounds.clone()),
            Loc::new(0, 7),
        );

        // arr[@1] + 1 - has DimPosition, blocks decomposition
        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(subscript),
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(10, 11))),
            Some(arr_bounds), // Has array bounds
            Loc::new(0, 11),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(add_expr)), None, Loc::new(0, 15));

        // Create A2A context with 2 dimensions
        let x_dim = Dimension::Indexed(CanonicalDimensionName::from_raw("x"), 5);
        let y_dim = Dimension::Indexed(CanonicalDimensionName::from_raw("y"), 3);
        let active_dimensions = vec![x_dim, y_dim];
        let active_subscripts = vec![
            CanonicalElementName::from_raw("3"), // x = 3
            CanonicalElementName::from_raw("2"), // y = 2
        ];

        let mut ctx = Pass1Context::with_a2a_context(&active_dimensions, &active_subscripts);
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // DimPosition should NOT be resolved and should block decomposition
        // because it's a dimension binding mechanism, not a simple lookup
        assert!(
            assignments.is_empty(),
            "DimPosition should block decomposition even with A2A context"
        );

        // The DimPosition should remain unchanged
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => {
                match *inner {
                    Expr3::Op2(BinaryOp::Add, ref left, _, _, _) => {
                        match left.as_ref() {
                            Expr3::Subscript(_, indices, _, _) => {
                                // DimPosition should still be there, not converted to Const
                                match &indices[0] {
                                    IndexExpr3::DimPosition(pos, _) => {
                                        assert_eq!(*pos, 1, "DimPosition should be preserved");
                                    }
                                    _ => panic!(
                                        "Expected DimPosition to be preserved, got {:?}",
                                        indices[0]
                                    ),
                                }
                            }
                            _ => panic!("Expected Subscript, got {:?}", left),
                        }
                    }
                    _ => panic!("Expected Op2, got {:?}", inner),
                }
            }
            _ => panic!("Expected App(Sum(...))"),
        }
    }

    #[test]
    fn test_pass0_element_name_takes_precedence_over_dimension() {
        // When an element name matches a dimension name, it should be treated
        // as an element reference (Expr), not a dimension reference (Dimension).
        //
        // Example: A dimension named "Region" has elements ["North", "South", "Row"]
        // where "Row" is also a dimension name. arr[Row] should use the element "Row",
        // not create a Dimension reference to the Row dimension.

        // Create a named dimension where one element matches another dimension name
        let region_dim = named_dim("Region", &["North", "South", "Row"]);

        let ctx = TestLowerContext::new()
            .with_var("arr", vec![region_dim])
            .with_dimension_name("Region")
            .with_dimension_name("Row"); // "Row" is also a dimension name

        // arr[Row] - Row is both an element of Region and a dimension name
        let expr2 = Expr2::Subscript(
            canonicalize("arr"),
            vec![IndexExpr2::Expr(Expr2::Var(
                canonicalize("Row"),
                None,
                Loc::new(4, 7),
            ))],
            None,
            Loc::new(0, 8),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        // Should NOT create a Dimension reference - should remain as Expr
        // because "Row" is an element of the parent dimension "Region"
        match &expr3 {
            Expr3::Subscript(_, indices, _, _) => {
                match &indices[0] {
                    IndexExpr3::Expr(inner) => {
                        // Good - it stayed as an expression, not converted to Dimension
                        match inner {
                            Expr3::Var(name, _, _) => {
                                assert_eq!(name.as_str(), "row");
                            }
                            _ => panic!("Expected Var, got {:?}", inner),
                        }
                    }
                    IndexExpr3::Dimension(name, _) => {
                        panic!(
                            "Element name 'Row' should take precedence over dimension name, \
                            but got Dimension({:?})",
                            name
                        );
                    }
                    other => panic!("Expected Expr or Dimension, got {:?}", other),
                }
            }
            _ => panic!("Expected Subscript, got {:?}", expr3),
        }

        // The expression should NOT reference an A2A dimension
        // (since it's an element reference, not a dimension reference)
        assert!(
            !expr3.references_a2a_dimension(),
            "Element reference should not count as A2A dimension reference"
        );
    }

    #[test]
    fn test_pass1_unmatched_dimension_stays_unresolved() {
        // When a Dimension ref doesn't match any active dimension, it should
        // remain unresolved (not be converted to a constant).
        //
        // This is important for cases where:
        // 1. The equation has multiple dimension references
        // 2. Some are resolved by A2A context, some aren't

        let arr_bounds = ArrayBounds::Temp {
            id: 0,
            dims: vec![3, 4],
            dim_names: Some(vec!["row".to_string(), "col".to_string()]),
        };

        // Create arr[Row, UnknownDim] - UnknownDim won't be in A2A context
        let subscript = Expr3::Subscript(
            canonicalize("arr"),
            vec![
                IndexExpr3::Dimension(CanonicalDimensionName::from_raw("row"), Loc::new(4, 7)),
                IndexExpr3::Dimension(
                    CanonicalDimensionName::from_raw("unknowndim"),
                    Loc::new(9, 19),
                ),
            ],
            Some(arr_bounds.clone()),
            Loc::new(0, 20),
        );

        let add_expr = Expr3::Op2(
            BinaryOp::Add,
            Box::new(subscript),
            Box::new(Expr3::Const("1".to_string(), 1.0, Loc::new(23, 24))),
            Some(arr_bounds),
            Loc::new(0, 24),
        );

        let sum_expr = Expr3::App(BuiltinFn::Sum(Box::new(add_expr)), None, Loc::new(0, 28));

        // A2A context only has Row, not UnknownDim
        let row_dim = Dimension::Indexed(CanonicalDimensionName::from_raw("row"), 3);
        let active_dimensions = vec![row_dim];
        let active_subscripts = vec![CanonicalElementName::from_raw("2")]; // Row = 2

        let mut ctx = Pass1Context::with_a2a_context(&active_dimensions, &active_subscripts);
        let result = ctx.transform(sum_expr);
        let assignments = ctx.take_assignments();

        // Should NOT decompose because UnknownDim is still unresolved
        assert!(
            assignments.is_empty(),
            "Should not decompose when dimension ref is unmatched"
        );

        // Check that Row was resolved but UnknownDim was not
        match result {
            Expr3::App(BuiltinFn::Sum(inner), _, _) => match *inner {
                Expr3::Op2(BinaryOp::Add, ref left, _, _, _) => match left.as_ref() {
                    Expr3::Subscript(_, indices, _, _) => {
                        // First index (Row) should be resolved to a constant
                        match &indices[0] {
                            IndexExpr3::Expr(Expr3::Const(_, val, _)) => {
                                assert_eq!(*val, 2.0, "Row should be resolved to 2");
                            }
                            _ => panic!("Expected Row to be resolved, got {:?}", indices[0]),
                        }

                        // Second index (UnknownDim) should remain as Dimension
                        match &indices[1] {
                            IndexExpr3::Dimension(name, _) => {
                                assert_eq!(
                                    name.as_str(),
                                    "unknowndim",
                                    "UnknownDim should stay unresolved"
                                );
                            }
                            _ => panic!(
                                "Expected UnknownDim to remain as Dimension, got {:?}",
                                indices[1]
                            ),
                        }
                    }
                    _ => panic!("Expected Subscript, got {:?}", left),
                },
                _ => panic!("Expected Op2, got {:?}", inner),
            },
            _ => panic!("Expected App(Sum(...))"),
        }
    }
}
