// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::array_view::ArrayView;
use crate::ast::expr0::{BinaryOp, UnaryOp};
use crate::ast::expr2::ArrayBounds;
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{Canonical, CanonicalDimensionName, Ident};

/// Index expression for Expr3 subscripts.
/// Similar to IndexExpr2 but with Expr3 children.
#[allow(dead_code)]
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr3 {
    Wildcard(Loc),
    StarRange(CanonicalDimensionName, Loc),
    Range(Expr3, Expr3, Loc),
    DimPosition(u32, Loc),
    Expr(Expr3),
}

impl IndexExpr3 {
    #[allow(dead_code)]
    pub fn get_loc(&self) -> Loc {
        match self {
            IndexExpr3::Wildcard(loc) => *loc,
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
            IndexExpr3::Wildcard(_) => IndexExpr3::Wildcard(loc),
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
            IndexExpr3::Wildcard(Loc::new(1, 2)).get_loc(),
            Loc::new(1, 2)
        );
        assert_eq!(
            IndexExpr3::DimPosition(1, Loc::new(3, 4)).get_loc(),
            Loc::new(3, 4)
        );
    }
}
