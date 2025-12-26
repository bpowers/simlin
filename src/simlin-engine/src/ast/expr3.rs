// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::array_view::ArrayView;
use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr2::{ArrayBounds, Expr2, IndexExpr2};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{Canonical, CanonicalDimensionName, EquationResult, Ident};
use crate::dimensions::Dimension;
use crate::eqn_err;

/// Index expression for Expr3 subscripts.
///
/// Unlike IndexExpr2, this type does NOT have a Wildcard variant.
/// During the expr2 → expr3 lowering pass, all wildcards are resolved
/// to explicit StarRange expressions based on the variable's dimensions.
#[allow(dead_code)]
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
}

impl IndexExpr3 {
    #[allow(dead_code)]
    pub fn get_loc(&self) -> Loc {
        match self {
            IndexExpr3::StarRange(_, loc) => *loc,
            IndexExpr3::Range(_, _, loc) => *loc,
            IndexExpr3::DimPosition(_, loc) => *loc,
            IndexExpr3::Expr(e) => e.get_loc(),
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
#[allow(dead_code)]
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

    // Array-specific variants
    /// Static subscript with precomputed view.
    /// (variable name, view into array, base offset of variable, location)
    StaticSubscript(Ident<Canonical>, ArrayView, usize, Loc),
    /// Reference to a temporary array
    TempArray(u32, ArrayView, Loc),
    /// Reference to a specific element of a temporary array
    TempArrayElement(u32, ArrayView, usize, Loc),
    /// Assign an expression result to temporary array storage
    AssignTemp(u32, Box<Expr3>, ArrayView),
}

impl Expr3 {
    #[allow(dead_code)]
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

    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn get_array_view(&self) -> Option<&ArrayView> {
        match self {
            Expr3::StaticSubscript(_, view, _, _) => Some(view),
            Expr3::TempArray(_, view, _) => Some(view),
            Expr3::TempArrayElement(_, view, _, _) => Some(view),
            Expr3::AssignTemp(_, _, view) => Some(view),
            _ => None,
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
pub trait Expr3LowerContext {
    /// Get the dimensions of a variable, or None if it's a scalar.
    fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>>;
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
    #[allow(dead_code)]
    pub fn from_index_expr2<C: Expr3LowerContext>(
        expr: &IndexExpr2,
        dim: Option<&Dimension>,
        ctx: &C,
    ) -> EquationResult<Self> {
        match expr {
            IndexExpr2::Wildcard(loc) => {
                // Wildcard must be resolved to the dimension at this position
                let dim = dim.ok_or(crate::common::EquationError {
                    start: loc.start,
                    end: loc.end,
                    code: crate::common::ErrorCode::CantSubscriptScalar,
                })?;
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
    #[allow(dead_code)]
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
    }

    impl TestLowerContext {
        fn new() -> Self {
            Self {
                dimensions: HashMap::new(),
            }
        }

        fn with_var(mut self, name: &str, dims: Vec<Dimension>) -> Self {
            self.dimensions.insert(name.to_string(), dims);
            self
        }
    }

    impl Expr3LowerContext for TestLowerContext {
        fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>> {
            self.dimensions.get(ident).cloned()
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
}
