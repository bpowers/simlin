// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr2::{ArrayBounds, Expr2, IndexExpr2};
use crate::ast::literal::Literal;
use crate::builtins::{ArgKind, BuiltinFn, Loc};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, EquationResult, Ident,
};
use crate::dimensions::Dimension;
use crate::eqn_err;
use std::cell::Cell;

/// Index expression for Expr3 subscripts.
///
/// Unlike IndexExpr2, this type does NOT have a Wildcard variant.
/// During the expr2 → expr3 lowering pass, all wildcards are resolved
/// to explicit StarRange expressions based on the variable's dimensions.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum IndexExpr3 {
    /// Star range (*:dim or dim.*) - preserves dimension for iteration.
    /// This includes both user-specified star ranges AND wildcards that
    /// were converted during lowering.
    StarRange(CanonicalDimensionName, Loc),
    /// Static range with compile-time known bounds (0-based start, 0-based exclusive end).
    /// Example: arr[2:5] becomes StaticRange(1, 5) (1-based to 0-based conversion happens during lowering)
    /// This can be fully resolved at compile time to construct an ArrayView.
    StaticRange(usize, usize, Loc),
    /// Dynamic range with runtime-evaluated bounds.
    /// Example: arr[start:end] where start/end are variables.
    /// Cannot be resolved at compile time; requires runtime view manipulation.
    Range(Expr3, Expr3, Loc),
    /// Dimension position reference (e.g., @1, @2)
    DimPosition(u32, Loc),
    /// General expression subscript
    Expr(Expr3),
    /// Active A2A dimension reference.
    /// When a dimension name appears as a subscript in an A2A context,
    /// the value depends on which A2A element is being evaluated. The variant
    /// carries it symbolically to `compiler::subscript::normalize_subscripts3`,
    /// which allocates one active axis per occurrence and resolves the element
    /// through the one mapped read.
    Dimension(CanonicalDimensionName, Loc),
}

impl IndexExpr3 {
    #[allow(dead_code)] // Used in pass 2
    pub fn get_loc(&self) -> Loc {
        match self {
            IndexExpr3::StarRange(_, loc) => *loc,
            IndexExpr3::StaticRange(_, _, loc) => *loc,
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
            IndexExpr3::StaticRange(_, _, _) => false, // Static ranges have no A2A refs
        }
    }
}

/// Expr3 is the intermediate expression representation between type-checked Expr2
/// and the final compiler::Expr.
///
/// It is a purely STRUCTURAL rewrite of `Expr2`: every wildcard is resolved to
/// an explicit star range and every bare array reference carries its subscripts,
/// so a later stage never has to ask a variable's dimensions again. Nothing is
/// materialized here -- there is no temp-array variant, because the fragment's
/// one materialization pass (`compiler::array_operand`) runs on `compiler::Expr`,
/// after subscript resolution, where views are concrete.
///
/// Key differences from compiler::Expr:
/// - Uses Ident<Canonical> for variable names (not usize offsets)
/// - Keeps string representation in Const for debugging
/// - No module-specific variants (EvalModule, ModuleInput)
/// - No assignment variants (AssignCurr, AssignNext)
///
/// `Eq` is derived for the reason spelled out on [`crate::ast::Expr0`], even
/// though this layer is not itself salsa-cached: it keeps all four layers under
/// one rule, so a float-bearing variant added here cannot be a bare `f64` and
/// then be copied down to a layer where it would matter.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum Expr3 {
    // Core variants (similar to Expr2)
    Const(String, Literal, Loc),
    Var(Ident<Canonical>, Option<ArrayBounds>, Loc),
    App(BuiltinFn<Expr3>, Option<ArrayBounds>, Loc),
    /// Dynamic subscript - indices computed at runtime
    Subscript(Ident<Canonical>, Vec<IndexExpr3>, Option<ArrayBounds>, Loc),
    Op1(UnaryOp, Box<Expr3>, Option<ArrayBounds>, Loc),
    Op2(BinaryOp, Box<Expr3>, Box<Expr3>, Option<ArrayBounds>, Loc),
    If(Box<Expr3>, Box<Expr3>, Box<Expr3>, Option<ArrayBounds>, Loc),
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
                // A lookup's table position is a graphical-function identity
                // resolved at lowering (`ArgKind::Table`), not a value this
                // walk reads an apply-to-all reference out of.
                builtin
                    .args_with_kinds()
                    .any(|(arg, kind)| kind != ArgKind::Table && arg.references_a2a_dimension())
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
    fn get_dimensions(&self, ident: &str) -> Option<&[Dimension]>;

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
                let dim = dim.ok_or(crate::common::EquationError::new(
                    crate::common::ErrorCode::CantSubscriptScalar,
                    loc.start,
                    loc.end,
                ))?;
                // Convert wildcard to star range with the parent dimension name.
                // For indexed dimensions like Dim(5), this becomes StarRange("dim").
                // For named dimensions like Cities{Boston,NYC,LA}, this becomes StarRange("cities").
                // The downstream compiler/evaluator must recognize that StarRange(parent_dim)
                // means "iterate over all elements" (equivalent to IndexOp::Wildcard).
                let dim_name = dim.canonical_name().clone();
                Ok(IndexExpr3::StarRange(dim_name, *loc))
            }
            IndexExpr2::StarRange(subdim_name, loc) => {
                // Explicit star range - pass through unchanged
                Ok(IndexExpr3::StarRange(subdim_name.clone(), *loc))
            }
            IndexExpr2::Range(start, end, loc) => {
                let start_expr = Expr3::from_expr2(start, ctx)?;
                let end_expr = Expr3::from_expr2(end, ctx)?;

                // Check if both bounds are constants - if so, create a StaticRange
                if let (Expr3::Const(_, start_val, _), Expr3::Const(_, end_val, _)) =
                    (&start_expr, &end_expr)
                {
                    // Convert 1-based indices to 0-based for StaticRange
                    // StaticRange stores (0-based start, 0-based exclusive end)
                    let start_0based = (start_val.value() as usize).saturating_sub(1);
                    let end_0based = end_val.value() as usize; // end is already exclusive in XMILE
                    return Ok(IndexExpr3::StaticRange(start_0based, end_0based, *loc));
                }

                // Dynamic range - bounds will be evaluated at runtime
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
                    let element_name = CanonicalElementName::from(ident);
                    let is_element_of_parent = dim
                        .map(|d| d.get_offset(&element_name).is_some())
                        .unwrap_or(false);

                    if !is_element_of_parent {
                        let canonical = CanonicalDimensionName::from(ident);
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
                        .map(|dim| IndexExpr3::StarRange(dim.canonical_name().clone(), *loc))
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
                let lowered_builtin = builtin.try_map_ref(|e| Expr3::from_expr2(e, ctx))?;
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
                if let Some(d) = dims
                    && args.len() > d.len()
                {
                    // Find the first out-of-bounds subscript for error location
                    let first_extra = &args[d.len()];
                    let extra_loc = first_extra.get_loc();
                    return eqn_err!(MismatchedDimensions, extra_loc.start, extra_loc.end);
                }

                let lowered_args: EquationResult<Vec<IndexExpr3>> = args
                    .iter()
                    .enumerate()
                    .map(|(i, arg)| {
                        let dim = dims.and_then(|d| d.get(i));
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
// Temp allocation
// ============================================================================

/// Issues the temp-array ids of one fragment -- one variable's one phase, the
/// unit `compiler::Var::new` lowers.
///
/// An id is final when issued; nothing downstream renumbers it. The sequence
/// is 0-based per fragment because assembly's `FragmentMerger` max-merges
/// fragment temp `t` onto shared slot `t` when it concatenates sequential
/// fragments (`TempStrategy::Recycle`). One pass draws from it --
/// `compiler::array_operand`, the fragment's one materialization pass -- which
/// is what makes the ids dense and distinct with no reconciliation.
///
/// [`Self::element_scopes`] refines the plain counter: an apply-to-all or
/// arrayed equation evaluates its elements one after another, so a temp read
/// only inside one element's code is dead before the next element runs and the
/// elements share one id range. This is the recycling `FragmentMerger` performs
/// across fragments, one level down, and it is what keeps a 300-element reducer
/// equation at one temp slot rather than 300 (the bytecode `TempId` is a `u8`).
/// A temp several elements READ is allocated ahead of that range instead, so an
/// element cannot clobber it.
///
/// `count()` is the number of distinct ids the fragment has issued, which is
/// the length of its `temp_sizes` table.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Default)]
pub struct TempAllocator {
    /// The id the next `alloc` returns.
    next: Cell<u32>,
    /// One past the highest id issued and kept.
    count: Cell<u32>,
}

impl TempAllocator {
    /// The next id in the current scope.
    pub fn alloc(&self) -> u32 {
        let id = self.next.get();
        self.next.set(id + 1);
        if id + 1 > self.count.get() {
            self.count.set(id + 1);
        }
        id
    }

    /// The number of distinct ids issued and kept: the fragment's temp count.
    pub fn count(&self) -> u32 {
        self.count.get()
    }

    /// Begin a run of element scopes sharing one id range; see
    /// [`ElementScopes`].
    pub fn element_scopes(&self) -> ElementScopes<'_> {
        ElementScopes {
            temps: self,
            start: self.next.get(),
        }
    }
}

/// One id range shared by the sequential element lowerings of one equation.
///
/// [`Self::begin_element`] rewinds the allocator to the range's start, so the
/// next element reuses the ids the previous one consumed. Dropping the guard
/// moves the allocator past every id any element used: a pass that later
/// splices temps into the element code (the computed-operand materializer)
/// then aliases none of them, while each element's own temps stay live only
/// until that element's assignment.
#[must_use]
pub struct ElementScopes<'a> {
    temps: &'a TempAllocator,
    start: u32,
}

impl ElementScopes<'_> {
    /// Rewind to the shared range's start for the next element.
    pub fn begin_element(&self) {
        self.temps.next.set(self.start);
    }
}

impl Drop for ElementScopes<'_> {
    fn drop(&mut self) {
        self.temps.next.set(self.temps.count.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Ident;

    #[test]
    fn test_expr3_const() {
        let expr = Expr3::Const("42".to_string(), Literal::new(42.0), Loc::new(0, 2));
        assert_eq!(expr.get_loc(), Loc::new(0, 2));
        assert!(expr.get_array_bounds().is_none());
    }

    #[test]
    fn test_expr3_var_scalar() {
        let expr = Expr3::Var(Ident::new("x"), None, Loc::new(0, 1));
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
        let expr = Expr3::Var(Ident::new("arr"), Some(bounds), Loc::new(0, 3));
        assert!(expr.get_array_bounds().is_some());
        assert_eq!(expr.get_array_bounds().unwrap().dims(), &[3, 4]);
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

        let indexed_elements: crate::common::IdentMap<CanonicalElementName, usize> =
            canonical_elements
                .iter()
                .enumerate()
                .map(|(i, e)| (e.clone(), i))
                .collect();

        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                elements: canonical_elements,
                indexed_elements,
                maps_to: None,
                mappings: vec![],
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
        fn get_dimensions(&self, ident: &str) -> Option<&[Dimension]> {
            self.dimensions.get(ident).map(|dims| dims.as_slice())
        }

        fn is_dimension_name(&self, ident: &str) -> bool {
            self.dimension_names.contains(&ident.to_lowercase())
        }
    }

    #[test]
    fn test_lower_scalar_var() {
        let ctx = TestLowerContext::new();
        let expr2 = Expr2::Var(Ident::new("x"), None, Loc::new(0, 1));

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
        let expr2 = Expr2::Var(Ident::new("arr"), Some(bounds), Loc::new(0, 3));

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
            Ident::new("vec"),
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
            Ident::new("arr"),
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
            Ident::new("scalar"),
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
            Ident::new("matrix"),
            vec![
                IndexExpr2::Wildcard(Loc::new(7, 8)),
                IndexExpr2::Expr(Expr2::Const(
                    "2".to_string(),
                    Literal::new(2.0),
                    Loc::new(10, 11),
                )),
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
                        assert_eq!(*val, Literal::new(2.0));
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
                Ident::new("arr1"),
                Some(bounds1),
                Loc::new(0, 4),
            )),
            Box::new(Expr2::Var(
                Ident::new("arr2"),
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
            Ident::new("sales"),
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
            Ident::new("cube"),
            vec![
                IndexExpr2::Wildcard(Loc::new(5, 6)),
                IndexExpr2::Wildcard(Loc::new(8, 9)),
                IndexExpr2::Expr(Expr2::Const(
                    "5".to_string(),
                    Literal::new(5.0),
                    Loc::new(11, 12),
                )),
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
                    IndexExpr3::Expr(Expr3::Const(_, val, _)) => {
                        assert_eq!(*val, Literal::new(5.0))
                    }
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
            Ident::new("matrix"),
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
            Ident::new("arr"),
            vec![IndexExpr2::Expr(Expr2::Var(
                Ident::new("MyDim"),
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
            Ident::new("arr"),
            vec![IndexExpr2::Expr(Expr2::Var(
                Ident::new("x"),
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
            Ident::new("arr"),
            vec![
                IndexExpr2::Wildcard(Loc::new(4, 5)),
                IndexExpr2::Expr(Expr2::Var(Ident::new("Row"), None, Loc::new(7, 10))),
            ],
            None,
            Loc::new(0, 11),
        );

        let expr3 = Expr3::from_expr2(&expr2, &ctx).unwrap();

        // The expression should reference an A2A dimension
        assert!(expr3.references_a2a_dimension());

        // arr[*, *] - no dimension reference
        let expr2_no_dim = Expr2::Subscript(
            Ident::new("arr"),
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
    // TempAllocator
    // =========================================================================

    #[test]
    fn alloc_issues_dense_ids_and_counts_them() {
        let temps = TempAllocator::default();
        assert_eq!(temps.count(), 0);
        assert_eq!(temps.alloc(), 0);
        assert_eq!(temps.alloc(), 1);
        assert_eq!(temps.alloc(), 2);
        assert_eq!(temps.count(), 3);
    }

    #[test]
    fn element_scopes_share_one_range_and_end_above_every_element() {
        let temps = TempAllocator::default();
        assert_eq!(temps.alloc(), 0);
        {
            let scopes = temps.element_scopes();
            scopes.begin_element();
            assert_eq!((temps.alloc(), temps.alloc()), (1, 2));
            scopes.begin_element();
            assert_eq!(
                temps.alloc(),
                1,
                "the second element reuses the first's ids"
            );
            scopes.begin_element();
            assert_eq!((temps.alloc(), temps.alloc(), temps.alloc()), (1, 2, 3));
            assert_eq!(temps.count(), 4, "count is the widest element's extent");
        }
        assert_eq!(
            temps.alloc(),
            4,
            "after the scopes end, ids continue above every element's"
        );
        assert_eq!(temps.count(), 5);
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
            Ident::new("arr"),
            vec![IndexExpr2::Expr(Expr2::Var(
                Ident::new("Row"),
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
}
