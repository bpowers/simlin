// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::result::Result as StdResult;

use lalrpop_util::ParseError;

pub use crate::builtins::Loc;

use crate::builtins::{
    BuiltinContents, BuiltinFn, UntypedBuiltinFn, is_0_arity_builtin_fn, walk_builtin_expr,
};
use crate::common::{ElementName, EquationError, EquationResult, Ident};
use crate::datamodel::Dimension;
use crate::eqn_err;
use crate::model::ScopeStage0;
use crate::token::LexerType;

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
        if self.end < self.start {
            0
        } else {
            self.end - self.start
        }
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
}

/// Expr0 represents a parsed equation, before any calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr0 {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(UntypedBuiltinFn<Expr0>, Loc),
    Subscript(Ident, Vec<IndexExpr0>, Loc),
    Op1(UnaryOp, Box<Expr0>, Loc),
    Op2(BinaryOp, Box<Expr0>, Box<Expr0>, Loc),
    If(Box<Expr0>, Box<Expr0>, Box<Expr0>, Loc),
}

#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr0 {
    Wildcard(Loc),
    StarRange(Ident, Loc),
    Range(Expr0, Expr0, Loc),
    Expr(Expr0),
}

impl IndexExpr0 {
    fn reify_0_arity_builtins(self) -> Self {
        match self {
            IndexExpr0::Wildcard(_) => self,
            IndexExpr0::StarRange(_, _) => self,
            IndexExpr0::Range(_, _, _) => self,
            IndexExpr0::Expr(expr) => IndexExpr0::Expr(expr.reify_0_arity_builtins()),
        }
    }

    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            IndexExpr0::Wildcard(_loc) => IndexExpr0::Wildcard(loc),
            IndexExpr0::StarRange(d, _loc) => IndexExpr0::StarRange(d, loc),
            IndexExpr0::Range(l, r, _loc) => IndexExpr0::Range(l.strip_loc(), r.strip_loc(), loc),
            IndexExpr0::Expr(e) => IndexExpr0::Expr(e.strip_loc()),
        }
    }
}

impl Expr0 {
    /// new returns a new Expression AST if one can be constructed, or a list of
    /// source/equation errors if one couldn't be constructed.
    pub fn new(eqn: &str, lexer_type: LexerType) -> StdResult<Option<Expr0>, Vec<EquationError>> {
        let mut errs = Vec::new();

        let lexer = crate::token::Lexer::new(eqn, lexer_type);
        match crate::equation::EquationParser::new().parse(eqn, lexer) {
            Ok(ast) => Ok(Some(match lexer_type {
                // in variable equations we want to treat `pi` or `time`
                // as calls to `pi()` or `time()` builtin functions.  But
                // in unit equations we might have a unit called "time", and
                // function calls don't make sense there anyway.  So only
                // reify for definitions/equations.
                LexerType::Equation => ast.reify_0_arity_builtins(),
                LexerType::Units => ast,
            })),
            Err(err) => {
                use crate::common::ErrorCode::*;
                let err = match err {
                    ParseError::InvalidToken { location: l } => EquationError {
                        start: l as u16,
                        end: (l + 1) as u16,
                        code: InvalidToken,
                    },
                    ParseError::UnrecognizedEof {
                        location: l,
                        expected: _e,
                    } => {
                        // if we get an EOF at position 0, that simply means
                        // we have an empty (or comment-only) equation.  Its not
                        // an _error_, but we also don't have an AST
                        if l == 0 {
                            return Ok(None);
                        }
                        // TODO: we can give a more precise error message here, including what
                        //   types of tokens would be ok
                        EquationError {
                            start: l as u16,
                            end: (l + 1) as u16,
                            code: UnrecognizedEof,
                        }
                    }
                    ParseError::UnrecognizedToken {
                        token: (l, _t, r), ..
                    } => EquationError {
                        start: l as u16,
                        end: r as u16,
                        code: UnrecognizedToken,
                    },
                    ParseError::ExtraToken {
                        token: (l, _t, r), ..
                    } => EquationError {
                        start: l as u16,
                        end: r as u16,
                        code: ExtraToken,
                    },
                    ParseError::User { error: e } => e,
                };

                errs.push(err);

                Err(errs)
            }
        }
    }

    /// reify turns variable references to known 0-arity builtin functions
    /// like `pi()` into App()s of those functions.
    fn reify_0_arity_builtins(self) -> Self {
        match self {
            Expr0::Var(ref id, loc) => {
                if is_0_arity_builtin_fn(id) {
                    Expr0::App(UntypedBuiltinFn(id.clone(), vec![]), loc)
                } else {
                    self
                }
            }
            Expr0::Const(_, _, _) => self,
            Expr0::App(UntypedBuiltinFn(func, args), loc) => {
                let args = args
                    .into_iter()
                    .map(|arg| arg.reify_0_arity_builtins())
                    .collect::<Vec<_>>();
                Expr0::App(UntypedBuiltinFn(func, args), loc)
            }
            Expr0::Subscript(id, args, loc) => {
                let args = args
                    .into_iter()
                    .map(|arg| arg.reify_0_arity_builtins())
                    .collect::<Vec<_>>();
                Expr0::Subscript(id, args, loc)
            }
            Expr0::Op1(op, mut r, loc) => {
                *r = r.reify_0_arity_builtins();
                Expr0::Op1(op, r, loc)
            }
            Expr0::Op2(op, mut l, mut r, loc) => {
                *l = l.reify_0_arity_builtins();
                *r = r.reify_0_arity_builtins();
                Expr0::Op2(op, l, r, loc)
            }
            Expr0::If(mut cond, mut t, mut f, loc) => {
                *cond = cond.reify_0_arity_builtins();
                *t = t.reify_0_arity_builtins();
                *f = f.reify_0_arity_builtins();
                Expr0::If(cond, t, f, loc)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr0::Const(s, n, _loc) => Expr0::Const(s, n, loc),
            Expr0::Var(v, _loc) => Expr0::Var(v, loc),
            Expr0::App(UntypedBuiltinFn(builtin, args), _loc) => Expr0::App(
                UntypedBuiltinFn(
                    builtin,
                    args.into_iter().map(|arg| arg.strip_loc()).collect(),
                ),
                loc,
            ),
            Expr0::Subscript(off, subscripts, _) => {
                let subscripts = subscripts
                    .into_iter()
                    .map(|expr| expr.strip_loc())
                    .collect();
                Expr0::Subscript(off, subscripts, loc)
            }
            Expr0::Op1(op, r, _loc) => Expr0::Op1(op, Box::new(r.strip_loc()), loc),
            Expr0::Op2(op, l, r, _loc) => {
                Expr0::Op2(op, Box::new(l.strip_loc()), Box::new(r.strip_loc()), loc)
            }
            Expr0::If(cond, t, f, _loc) => Expr0::If(
                Box::new(cond.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                loc,
            ),
        }
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr0::Const(_, _, loc) => *loc,
            Expr0::Var(_, loc) => *loc,
            Expr0::App(_, loc) => *loc,
            Expr0::Subscript(_, _, loc) => *loc,
            Expr0::Op1(_, _, loc) => *loc,
            Expr0::Op2(_, _, _, loc) => *loc,
            Expr0::If(_, _, _, loc) => *loc,
        }
    }
}

/// IndexExpr1 represents a parsed equation index, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr1 {
    Wildcard(Loc),
    // *:dimension_name
    StarRange(Ident, Loc),
    Range(Expr1, Expr1, Loc),
    Expr(Expr1),
}

impl IndexExpr1 {
    pub(crate) fn from(expr: IndexExpr0) -> EquationResult<Self> {
        let expr = match expr {
            IndexExpr0::Wildcard(loc) => IndexExpr1::Wildcard(loc),
            IndexExpr0::StarRange(ident, loc) => IndexExpr1::StarRange(ident, loc),
            IndexExpr0::Range(l, r, loc) => {
                IndexExpr1::Range(Expr1::from(l)?, Expr1::from(r)?, loc)
            }
            IndexExpr0::Expr(e) => IndexExpr1::Expr(Expr1::from(e)?),
        };

        Ok(expr)
    }

    pub(crate) fn constify_dimensions(self, scope: &ScopeStage0) -> Self {
        match self {
            IndexExpr1::Wildcard(loc) => IndexExpr1::Wildcard(loc),
            IndexExpr1::StarRange(id, loc) => IndexExpr1::StarRange(id, loc),
            IndexExpr1::Range(l, r, loc) => IndexExpr1::Range(
                l.constify_dimensions(scope),
                r.constify_dimensions(scope),
                loc,
            ),
            IndexExpr1::Expr(e) => IndexExpr1::Expr(e.constify_dimensions(scope)),
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            IndexExpr1::Wildcard(_) => None,
            IndexExpr1::StarRange(v, loc) => {
                if v == ident {
                    Some(*loc)
                } else {
                    None
                }
            }
            IndexExpr1::Range(l, r, _) => {
                if let Some(loc) = l.get_var_loc(ident) {
                    return Some(loc);
                }
                r.get_var_loc(ident)
            }
            IndexExpr1::Expr(e) => e.get_var_loc(ident),
        }
    }
}

impl Default for Expr0 {
    fn default() -> Self {
        Expr0::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

/// Expr1 represents a parsed equation, after calls to
/// builtin functions have been checked/resolved.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr1 {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(BuiltinFn<Expr1>, Loc),
    Subscript(Ident, Vec<IndexExpr1>, Loc),
    Op1(UnaryOp, Box<Expr1>, Loc),
    Op2(BinaryOp, Box<Expr1>, Box<Expr1>, Loc),
    If(Box<Expr1>, Box<Expr1>, Box<Expr1>, Loc),
}

impl Expr1 {
    pub(crate) fn from(expr: Expr0) -> EquationResult<Self> {
        let expr = match expr {
            Expr0::Const(s, n, loc) => Expr1::Const(s, n, loc),
            Expr0::Var(id, loc) => Expr1::Var(id, loc),
            Expr0::App(UntypedBuiltinFn(id, orig_args), loc) => {
                let args: EquationResult<Vec<Expr1>> =
                    orig_args.into_iter().map(Expr1::from).collect();
                let mut args = args?;

                macro_rules! check_arity {
                    ($builtin_fn:tt, 0) => {{
                        if !args.is_empty() {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        BuiltinFn::$builtin_fn
                    }};
                    ($builtin_fn:tt, 1) => {{
                        if args.len() != 1 {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a))
                    }};
                    ($builtin_fn:tt, 2) => {{
                        if args.len() != 2 {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        let b = args.remove(1);
                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a), Box::new(b))
                    }};
                    ($builtin_fn:tt, 1, 2) => {{
                        if args.len() == 1 {
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), None)
                        } else if args.len() == 2 {
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Some(Box::new(b)))
                        } else {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }
                    }};
                    ($builtin_fn:tt, 3) => {{
                        if args.len() != 3 {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }

                        let c = args.remove(2);
                        let b = args.remove(1);
                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), Box::new(c))
                    }};
                    ($builtin_fn:tt, 1, 3) => {{
                        if args.len() == 1 {
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), None)
                        } else if args.len() == 2 {
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Some((Box::new(b), None)))
                        } else if args.len() == 3 {
                            let c = args.remove(2);
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(
                                Box::new(a),
                                Some((Box::new(b), Some(Box::new(c)))),
                            )
                        } else {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }
                    }};
                    ($builtin_fn:tt, 2, 3) => {{
                        if args.len() == 2 {
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), None)
                        } else if args.len() == 3 {
                            let c = args.remove(2);
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), Some(Box::new(c)))
                        } else {
                            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
                        }
                    }};
                }

                let builtin = match id.as_str() {
                    "lookup" => {
                        if let Some(Expr1::Var(ident, loc)) = args.first() {
                            BuiltinFn::Lookup(ident.clone(), Box::new(args[1].clone()), *loc)
                        } else {
                            return eqn_err!(BadTable, loc.start, loc.end);
                        }
                    }
                    "mean" => BuiltinFn::Mean(args),
                    "abs" => check_arity!(Abs, 1),
                    "arccos" => check_arity!(Arccos, 1),
                    "arcsin" => check_arity!(Arcsin, 1),
                    "arctan" => check_arity!(Arctan, 1),
                    "cos" => check_arity!(Cos, 1),
                    "exp" => check_arity!(Exp, 1),
                    "inf" => check_arity!(Inf, 0),
                    "int" => check_arity!(Int, 1),
                    "ismoduleinput" => {
                        if let Some(Expr1::Var(ident, loc)) = args.first() {
                            BuiltinFn::IsModuleInput(ident.clone(), *loc)
                        } else {
                            return eqn_err!(ExpectedIdent, loc.start, loc.end);
                        }
                    }
                    "ln" => check_arity!(Ln, 1),
                    "log10" => check_arity!(Log10, 1),
                    "max" => check_arity!(Max, 1, 2),
                    "min" => check_arity!(Min, 1, 2),
                    "pi" => check_arity!(Pi, 0),
                    "pulse" => check_arity!(Pulse, 2, 3),
                    "ramp" => check_arity!(Ramp, 2, 3),
                    "safediv" => check_arity!(SafeDiv, 2, 3),
                    "sin" => check_arity!(Sin, 1),
                    "sqrt" => check_arity!(Sqrt, 1),
                    "step" => check_arity!(Step, 2),
                    "tan" => check_arity!(Tan, 1),
                    "time" => check_arity!(Time, 0),
                    "time_step" | "dt" => check_arity!(TimeStep, 0),
                    "initial_time" => check_arity!(StartTime, 0),
                    "final_time" => check_arity!(FinalTime, 0),
                    "rank" => check_arity!(Rank, 1, 3),
                    "size" => check_arity!(Size, 1),
                    "stddev" => check_arity!(Stddev, 1),
                    "sum" => check_arity!(Sum, 1),
                    _ => {
                        // TODO: this could be a table reference, array reference,
                        //       or module instantiation according to 3.3.2 of the spec
                        return eqn_err!(UnknownBuiltin, loc.start, loc.end);
                    }
                };
                Expr1::App(builtin, loc)
            }
            Expr0::Subscript(id, args, loc) => {
                let args: EquationResult<Vec<IndexExpr1>> =
                    args.into_iter().map(IndexExpr1::from).collect();
                Expr1::Subscript(id, args?, loc)
            }
            Expr0::Op1(op, l, loc) => Expr1::Op1(op, Box::new(Expr1::from(*l)?), loc),
            Expr0::Op2(op, l, r, loc) => Expr1::Op2(
                op,
                Box::new(Expr1::from(*l)?),
                Box::new(Expr1::from(*r)?),
                loc,
            ),
            Expr0::If(cond, t, f, loc) => Expr1::If(
                Box::new(Expr1::from(*cond)?),
                Box::new(Expr1::from(*t)?),
                Box::new(Expr1::from(*f)?),
                loc,
            ),
        };
        Ok(expr)
    }

    pub(crate) fn constify_dimensions(self, scope: &ScopeStage0) -> Self {
        match self {
            Expr1::Const(s, n, loc) => Expr1::Const(s, n, loc),
            Expr1::Var(id, loc) => {
                if let Some(off) = scope.dimensions.lookup(&id) {
                    Expr1::Const(id, off as f64, loc)
                } else {
                    Expr1::Var(id, loc)
                }
            }
            Expr1::App(func, loc) => {
                let func = match func {
                    BuiltinFn::Inf => BuiltinFn::Inf,
                    BuiltinFn::Pi => BuiltinFn::Pi,
                    BuiltinFn::Time => BuiltinFn::Time,
                    BuiltinFn::TimeStep => BuiltinFn::TimeStep,
                    BuiltinFn::StartTime => BuiltinFn::StartTime,
                    BuiltinFn::FinalTime => BuiltinFn::FinalTime,
                    BuiltinFn::Abs(a) => BuiltinFn::Abs(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Arccos(a) => {
                        BuiltinFn::Arccos(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Arcsin(a) => {
                        BuiltinFn::Arcsin(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Arctan(a) => {
                        BuiltinFn::Arctan(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Cos(a) => BuiltinFn::Cos(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Exp(a) => BuiltinFn::Exp(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Int(a) => BuiltinFn::Int(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Ln(a) => BuiltinFn::Ln(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Log10(a) => BuiltinFn::Log10(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Sin(a) => BuiltinFn::Sin(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Tan(a) => BuiltinFn::Tan(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Mean(args) => BuiltinFn::Mean(
                        args.into_iter()
                            .map(|arg| arg.constify_dimensions(scope))
                            .collect(),
                    ),
                    BuiltinFn::Max(a, b) => BuiltinFn::Max(
                        Box::new(a.constify_dimensions(scope)),
                        b.map(|expr| Box::new(expr.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Min(a, b) => BuiltinFn::Min(
                        Box::new(a.constify_dimensions(scope)),
                        b.map(|expr| Box::new(expr.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Step(a, b) => BuiltinFn::Step(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                    ),
                    BuiltinFn::IsModuleInput(id, loc) => BuiltinFn::IsModuleInput(id, loc),
                    BuiltinFn::Lookup(id, arg, loc) => {
                        BuiltinFn::Lookup(id, Box::new(arg.constify_dimensions(scope)), loc)
                    }
                    BuiltinFn::Pulse(a, b, c) => BuiltinFn::Pulse(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                        c.map(|arg| Box::new(arg.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Ramp(a, b, c) => BuiltinFn::Ramp(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                        c.map(|arg| Box::new(arg.constify_dimensions(scope))),
                    ),
                    BuiltinFn::SafeDiv(a, b, c) => BuiltinFn::SafeDiv(
                        Box::new(a.constify_dimensions(scope)),
                        Box::new(b.constify_dimensions(scope)),
                        c.map(|arg| Box::new(arg.constify_dimensions(scope))),
                    ),
                    BuiltinFn::Rank(a, rest) => BuiltinFn::Rank(
                        Box::new(a.constify_dimensions(scope)),
                        rest.map(|(b, c)| {
                            (
                                Box::new(b.constify_dimensions(scope)),
                                c.map(|c| Box::new(c.constify_dimensions(scope))),
                            )
                        }),
                    ),
                    BuiltinFn::Size(a) => BuiltinFn::Size(Box::new(a.constify_dimensions(scope))),
                    BuiltinFn::Stddev(a) => {
                        BuiltinFn::Stddev(Box::new(a.constify_dimensions(scope)))
                    }
                    BuiltinFn::Sum(a) => BuiltinFn::Sum(Box::new(a.constify_dimensions(scope))),
                };
                Expr1::App(func, loc)
            }
            Expr1::Subscript(id, args, loc) => Expr1::Subscript(
                id,
                args.into_iter()
                    .map(|arg| arg.constify_dimensions(scope))
                    .collect(),
                loc,
            ),
            Expr1::Op1(op, l, loc) => Expr1::Op1(op, Box::new(l.constify_dimensions(scope)), loc),
            Expr1::Op2(op, l, r, loc) => Expr1::Op2(
                op,
                Box::new(l.constify_dimensions(scope)),
                Box::new(r.constify_dimensions(scope)),
                loc,
            ),
            Expr1::If(cond, l, r, loc) => Expr1::If(
                Box::new(cond.constify_dimensions(scope)),
                Box::new(l.constify_dimensions(scope)),
                Box::new(r.constify_dimensions(scope)),
                loc,
            ),
        }
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr1::Const(_, _, loc) => *loc,
            Expr1::Var(_, loc) => *loc,
            Expr1::App(_, loc) => *loc,
            Expr1::Subscript(_, _, loc) => *loc,
            Expr1::Op1(_, _, loc) => *loc,
            Expr1::Op2(_, _, _, loc) => *loc,
            Expr1::If(_, _, _, loc) => *loc,
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr1::Const(_s, _n, _loc) => None,
            Expr1::Var(v, loc) if v == ident => Some(*loc),
            Expr1::Var(_v, _loc) => None,
            Expr1::App(builtin, _loc) => {
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
            Expr1::Subscript(id, subscripts, loc) => {
                if id == ident {
                    let start = loc.start as usize;
                    return Some(Loc::new(start, start + id.len()));
                }
                for arg in subscripts.iter() {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr1::Op1(_op, r, _loc) => r.get_var_loc(ident),
            Expr1::Op2(_op, l, r, _loc) => l.get_var_loc(ident).or_else(|| r.get_var_loc(ident)),
            Expr1::If(cond, t, f, _loc) => cond
                .get_var_loc(ident)
                .or_else(|| t.get_var_loc(ident))
                .or_else(|| f.get_var_loc(ident)),
        }
    }
}

impl Default for Expr1 {
    fn default() -> Self {
        Expr1::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

/// Expr represents a dimension-annotated expression, the final stage
/// of AST transformation with full dimension information.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(String, f64, DimensionVec, Loc),
    Var(Ident, DimensionVec, Loc),
    App(BuiltinFn<Expr>, DimensionVec, Loc),
    Subscript(Ident, Vec<IndexExpr>, DimensionVec, Loc),
    Op1(UnaryOp, Box<Expr>, DimensionVec, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, DimensionVec, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, DimensionVec, Loc),
}

/// IndexExpr represents a dimension-annotated index expression.
#[derive(PartialEq, Clone, Debug)]
pub enum IndexExpr {
    Wildcard(DimensionVec, Loc),
    StarRange(Ident, DimensionVec, Loc),
    Range(Expr, Expr, DimensionVec, Loc),
    Expr(Expr),
}

impl Expr {
    /// Get the dimensions of this expression
    pub fn dims(&self) -> &DimensionVec {
        match self {
            Expr::Const(_, _, dims, _) => dims,
            Expr::Var(_, dims, _) => dims,
            Expr::App(_, dims, _) => dims,
            Expr::Subscript(_, _, dims, _) => dims,
            Expr::Op1(_, _, dims, _) => dims,
            Expr::Op2(_, _, _, dims, _) => dims,
            Expr::If(_, _, _, dims, _) => dims,
        }
    }

    /// Get the location of this expression
    pub fn loc(&self) -> Loc {
        match self {
            Expr::Const(_, _, _, loc) => *loc,
            Expr::Var(_, _, loc) => *loc,
            Expr::App(_, _, loc) => *loc,
            Expr::Subscript(_, _, _, loc) => *loc,
            Expr::Op1(_, _, _, loc) => *loc,
            Expr::Op2(_, _, _, _, loc) => *loc,
            Expr::If(_, _, _, _, loc) => *loc,
        }
    }
}

impl IndexExpr {
    /// Get the dimensions of this index expression
    pub fn dims(&self) -> &DimensionVec {
        match self {
            IndexExpr::Wildcard(dims, _) => dims,
            IndexExpr::StarRange(_, dims, _) => dims,
            IndexExpr::Range(_, _, dims, _) => dims,
            IndexExpr::Expr(expr) => expr.dims(),
        }
    }
}

/// Context for dimension inference
pub struct DimensionContext<'a> {
    /// Variable dimensions from the model
    pub var_dims: &'a HashMap<Ident, DimensionVec>,
}

impl IndexExpr {
    /// Infer dimensions for index expressions
    fn infer_dimensions(expr: IndexExpr1, ctx: &DimensionContext) -> EquationResult<Self> {
        match expr {
            IndexExpr1::Wildcard(loc) => Ok(IndexExpr::Wildcard(DimensionVec::scalar(), loc)),
            IndexExpr1::StarRange(ident, loc) => {
                Ok(IndexExpr::StarRange(ident, DimensionVec::scalar(), loc))
            }
            IndexExpr1::Range(start, end, loc) => {
                let start = Expr::infer_dimensions(start, ctx)?;
                let end = Expr::infer_dimensions(end, ctx)?;
                Ok(IndexExpr::Range(start, end, DimensionVec::scalar(), loc))
            }
            IndexExpr1::Expr(e) => {
                let e = Expr::infer_dimensions(e, ctx)?;
                Ok(IndexExpr::Expr(e))
            }
        }
    }
}

impl Expr {
    /// Infer dimensions from an Expr1 to create a dimension-annotated Expr
    pub fn infer_dimensions(expr: Expr1, ctx: &DimensionContext) -> EquationResult<Self> {
        let result = match expr {
            Expr1::Const(s, n, loc) => {
                // Constants are always scalar
                Expr::Const(s, n, DimensionVec::scalar(), loc)
            }
            Expr1::Var(id, loc) => {
                // Look up variable dimensions from context
                let dims = ctx
                    .var_dims
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(DimensionVec::scalar);
                Expr::Var(id, dims, loc)
            }
            Expr1::App(builtin, loc) => {
                // Infer dimensions for builtin functions
                let (builtin, dims) = Self::infer_builtin_dimensions(builtin, ctx)?;
                Expr::App(builtin, dims, loc)
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
                    .map(|idx| IndexExpr::infer_dimensions(idx, ctx))
                    .collect();
                let index_exprs = index_exprs?;

                // Calculate resulting dimensions after subscripting
                let result_dims = Self::apply_subscript_to_dims(&base_dims, &index_exprs, loc)?;

                Expr::Subscript(id, index_exprs, result_dims, loc)
            }
            Expr1::Op1(op, expr, loc) => {
                let expr = Box::new(Self::infer_dimensions(*expr, ctx)?);
                // Unary operations preserve dimensions
                let dims = expr.dims().clone();
                Expr::Op1(op, expr, dims, loc)
            }
            Expr1::Op2(op, l, r, loc) => {
                let l = Box::new(Self::infer_dimensions(*l, ctx)?);
                let r = Box::new(Self::infer_dimensions(*r, ctx)?);

                // Infer dimensions based on operation type
                let dims = Self::infer_binary_op_dims(op, l.dims(), r.dims(), loc)?;

                Expr::Op2(op, l, r, dims, loc)
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

                Expr::If(cond, t, f, dims, loc)
            }
        };

        Ok(result)
    }

    /// Infer dimensions for builtin functions
    fn infer_builtin_dimensions(
        builtin: BuiltinFn<Expr1>,
        ctx: &DimensionContext,
    ) -> EquationResult<(BuiltinFn<Expr>, DimensionVec)> {
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
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Abs(a), dims))
            }
            Arccos(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Arccos(a), dims))
            }
            Arcsin(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Arcsin(a), dims))
            }
            Arctan(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Arctan(a), dims))
            }
            Cos(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Cos(a), dims))
            }
            Exp(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Exp(a), dims))
            }
            Int(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Int(a), dims))
            }
            Ln(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Ln(a), dims))
            }
            Log10(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Log10(a), dims))
            }
            Sin(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Sin(a), dims))
            }
            Sqrt(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Sqrt(a), dims))
            }
            Tan(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let dims = a.dims().clone();
                Ok((Tan(a), dims))
            }

            // Array reduction functions return scalars
            Mean(args) => {
                let args: Result<Vec<_>, _> = args
                    .into_iter()
                    .map(|arg| Expr::infer_dimensions(arg, ctx))
                    .collect();
                Ok((Mean(args?), DimensionVec::scalar()))
            }
            Sum(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                Ok((Sum(a), DimensionVec::scalar()))
            }
            Size(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                Ok((Size(a), DimensionVec::scalar()))
            }
            Stddev(a) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                Ok((Stddev(a), DimensionVec::scalar()))
            }

            // Min/Max with multiple arguments - result has broadcast shape
            Min(a, b_opt) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let mut dims = a.dims().clone();

                if let Some(b) = b_opt {
                    let b = Box::new(Expr::infer_dimensions(*b, ctx)?);
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
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let mut dims = a.dims().clone();

                if let Some(b) = b_opt {
                    let b = Box::new(Expr::infer_dimensions(*b, ctx)?);
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
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                Ok((Lookup(id, a, loc), DimensionVec::scalar()))
            }
            IsModuleInput(id, loc) => Ok((IsModuleInput(id, loc), DimensionVec::scalar())),
            Pulse(a, b, c) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr::infer_dimensions(*b, ctx)?);
                let c = match c {
                    Some(c_expr) => Some(Box::new(Expr::infer_dimensions(*c_expr, ctx)?)),
                    None => None,
                };
                Ok((Pulse(a, b, c), DimensionVec::scalar()))
            }
            Ramp(a, b, c) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr::infer_dimensions(*b, ctx)?);
                let c = match c {
                    Some(c_expr) => Some(Box::new(Expr::infer_dimensions(*c_expr, ctx)?)),
                    None => None,
                };
                Ok((Ramp(a, b, c), DimensionVec::scalar()))
            }
            SafeDiv(a, b, c) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr::infer_dimensions(*b, ctx)?);
                let c = match c {
                    Some(c_expr) => Some(Box::new(Expr::infer_dimensions(*c_expr, ctx)?)),
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
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let b = Box::new(Expr::infer_dimensions(*b, ctx)?);
                Ok((Step(a, b), DimensionVec::scalar()))
            }
            Rank(a, bc_opt) => {
                let a = Box::new(Expr::infer_dimensions(*a, ctx)?);
                let bc = match bc_opt {
                    Some((b, c_opt)) => {
                        let b = Box::new(Expr::infer_dimensions(*b, ctx)?);
                        let c = match c_opt {
                            Some(c_expr) => Some(Box::new(Expr::infer_dimensions(*c_expr, ctx)?)),
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
        indices: &[IndexExpr],
        loc: Loc,
    ) -> EquationResult<DimensionVec> {
        if indices.len() > base_dims.ndim() {
            return eqn_err!(MismatchedDimensions, loc.start, loc.end);
        }

        // Convert index expressions to SliceSpecs
        let mut slice_specs = Vec::new();
        for (_i, idx) in indices.iter().enumerate() {
            let spec = match idx {
                IndexExpr::Wildcard(_, _) => SliceSpec::Wildcard,
                IndexExpr::StarRange(name, _, _) => SliceSpec::DimName(name.clone()),
                IndexExpr::Range(_start, _end, _, _) => {
                    // For now, assume constant ranges
                    // In a real implementation, we'd need to evaluate these
                    SliceSpec::Range(0, 10) // Placeholder
                }
                IndexExpr::Expr(_) => SliceSpec::Index(0), // Placeholder
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

#[test]
fn test_parse() {
    use crate::ast::BinaryOp::*;
    use crate::ast::Expr0::*;

    let if1 = Box::new(If(
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Const("3".to_string(), 3.0, Loc::default())),
        Loc::default(),
    ));

    let if2 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("blerg".to_string(), Loc::default())),
            Box::new(Var("foo".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Const("3".to_string(), 3.0, Loc::default())),
        Loc::default(),
    ));

    let if3 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("quotient".to_string(), Loc::default())),
            Box::new(Var("quotient_target".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("0".to_string(), 0.0, Loc::default())),
        Loc::default(),
    ));

    let if4 = Box::new(If(
        Box::new(Op2(
            And,
            Box::new(Var("true_input".to_string(), Loc::default())),
            Box::new(Var("false_input".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("0".to_string(), 0.0, Loc::default())),
        Loc::default(),
    ));

    let quoting_eq = Box::new(Op2(
        Eq,
        Box::new(Var("oh_dear".to_string(), Loc::default())),
        Box::new(Var("oh_dear".to_string(), Loc::default())),
        Loc::default(),
    ));

    let subscript1 = Box::new(Subscript(
        "a".to_owned(),
        vec![IndexExpr0::Expr(Const("1".to_owned(), 1.0, Loc::default()))],
        Loc::default(),
    ));
    let subscript2 = Box::new(Subscript(
        "a".to_owned(),
        vec![
            IndexExpr0::Expr(Const("2".to_owned(), 2.0, Loc::default())),
            IndexExpr0::Expr(App(
                UntypedBuiltinFn("int".to_owned(), vec![Var("b".to_owned(), Loc::default())]),
                Loc::default(),
            )),
        ],
        Loc::default(),
    ));

    let subscript3 = Box::new(Subscript(
        "a".to_string(),
        vec![
            IndexExpr0::Wildcard(Loc::default()),
            IndexExpr0::Wildcard(Loc::default()),
        ],
        Loc::default(),
    ));

    let subscript4 = Box::new(Subscript(
        "a".to_string(),
        vec![IndexExpr0::StarRange("d".to_string(), Loc::default())],
        Loc::default(),
    ));

    let subscript5 = Box::new(Subscript(
        "a".to_string(),
        vec![IndexExpr0::Range(
            Const("1".to_owned(), 1.0, Loc::default()),
            Const("2".to_owned(), 2.0, Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    ));

    let subscript6 = Box::new(Subscript(
        "a".to_string(),
        vec![IndexExpr0::Range(
            Var("l".to_owned(), Loc::default()),
            Var("r".to_owned(), Loc::default()),
            Loc::default(),
        )],
        Loc::default(),
    ));

    let time1 = Box::new(App(
        UntypedBuiltinFn("time".to_owned(), vec![]),
        Loc::default(),
    ));

    let time2 = Box::new(Subscript(
        "aux".to_owned(),
        vec![IndexExpr0::Expr(Op2(
            BinaryOp::Add,
            Box::new(App(
                UntypedBuiltinFn(
                    "int".to_owned(),
                    vec![Op2(
                        BinaryOp::Mod,
                        Box::new(App(
                            UntypedBuiltinFn("time".to_owned(), vec![]),
                            Loc::default(),
                        )),
                        Box::new(Const("5".to_owned(), 5.0, Loc::default())),
                        Loc::default(),
                    )],
                ),
                Loc::default(),
            )),
            Box::new(Const("1".to_owned(), 1.0, Loc::default())),
            Loc::default(),
        ))],
        Loc::default(),
    ));

    let cases = [
        (
            "aux[INT(TIME MOD 5) + 1]",
            time2,
            "aux[int(time() mod 5) + 1]",
        ),
        ("if 1 then 2 else 3", if1, "if (1) then (2) else (3)"),
        (
            "if blerg = foo then 2 else 3",
            if2,
            "if (blerg = foo) then (2) else (3)",
        ),
        (
            "IF quotient = quotient_target THEN 1 ELSE 0",
            if3.clone(),
            "if (quotient = quotient_target) then (1) else (0)",
        ),
        (
            "(IF quotient = quotient_target THEN 1 ELSE 0)",
            if3,
            "if (quotient = quotient_target) then (1) else (0)",
        ),
        (
            "( IF true_input and false_input THEN 1 ELSE 0 )",
            if4.clone(),
            "if (true_input && false_input) then (1) else (0)",
        ),
        (
            "( IF true_input && false_input THEN 1 ELSE 0 )",
            if4,
            "if (true_input && false_input) then (1) else (0)",
        ),
        ("\"oh dear\" = oh_dear", quoting_eq, "oh_dear = oh_dear"),
        ("a[1]", subscript1, "a[1]"),
        ("a[2, INT(b)]", subscript2, "a[2, int(b)]"),
        ("time", time1, "time()"),
        ("a[*, *]", subscript3, "a[*, *]"),
        ("a[*:d]", subscript4, "a[*:d]"),
        ("a[1:2]", subscript5, "a[1:2]"),
        ("a[l:r]", subscript6, "a[l:r]"),
    ];

    for case in cases.iter() {
        let eqn = case.0;
        let ast = Expr0::new(eqn, LexerType::Equation).unwrap();
        assert!(ast.is_some());
        let ast = ast.unwrap().strip_loc();
        assert_eq!(&*case.1, &ast);
        let printed = print_eqn(&ast);
        assert_eq!(case.2, &printed);
    }

    let ast = Expr0::new("NAN", LexerType::Equation).unwrap();
    assert!(ast.is_some());
    let ast = ast.unwrap();
    assert!(matches!(&ast, Expr0::Const(_, _, _)));
    if let Expr0::Const(id, n, _) = &ast {
        assert_eq!("NaN", id);
        assert!(n.is_nan());
    }
    let printed = print_eqn(&ast);
    assert_eq!("NaN", &printed);
}

#[test]
fn test_parse_failures() {
    let failures = &[
        "(",
        "(3",
        "3 +",
        "3 *",
        "(3 +)",
        "call(a,",
        "call(a,1+",
        "if if",
        "if 1 then",
        "if then",
        "if 1 then 2 else",
        "a[*:2]",
        "a[2:*]",
        "a[b:*]",
        "a[*:]",
        "a[3:]",
    ];

    for case in failures {
        let err = Expr0::new(case, LexerType::Equation).unwrap_err();
        assert!(!err.is_empty());
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ast<T> {
    Scalar(T),
    ApplyToAll(Vec<Dimension>, T),
    Arrayed(Vec<Dimension>, HashMap<ElementName, T>),
}

impl Ast<Expr1> {
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Ast::Scalar(expr) => expr.get_var_loc(ident),
            Ast::ApplyToAll(_, expr) => expr.get_var_loc(ident),
            Ast::Arrayed(_, subscripts) => {
                for (_, expr) in subscripts.iter() {
                    if let Some(loc) = expr.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
        }
    }

    pub fn to_latex(&self) -> String {
        match self {
            Ast::Scalar(expr) => latex_eqn(expr),
            Ast::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            Ast::Arrayed(_, _) => "TODO(array)".to_owned(),
        }
    }
}

pub(crate) fn lower_ast(scope: &ScopeStage0, ast: Ast<Expr0>) -> EquationResult<Ast<Expr1>> {
    match ast {
        Ast::Scalar(expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(Ast::Scalar),
        Ast::ApplyToAll(dims, expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .map(|expr| Ast::ApplyToAll(dims, expr)),
        Ast::Arrayed(dims, elements) => {
            let elements: EquationResult<HashMap<ElementName, Expr1>> = elements
                .into_iter()
                .map(|(id, expr)| {
                    match Expr1::from(expr).map(|expr| expr.constify_dimensions(scope)) {
                        Ok(expr) => Ok((id, expr)),
                        Err(err) => Err(err),
                    }
                })
                .collect();
            match elements {
                Ok(elements) => Ok(Ast::Arrayed(dims, elements)),
                Err(err) => Err(err),
            }
        }
    }
}

/// Visitors walk Expr ASTs.
pub trait Visitor<T> {
    fn walk_index(&mut self, e: &IndexExpr0) -> T;
    fn walk(&mut self, e: &Expr0) -> T;
}

/// BinaryOp enumerates the different operators supported in
/// system dynamics equations.
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

macro_rules! child_needs_parens(
    ($expr:tt, $parent:expr, $child:expr, $eqn:expr) => {{
        match $parent {
            // no children so doesn't matter
            $expr::Const(_, _, _) | $expr::Var(_, _) => false,
            // children are comma separated, so no ambiguity possible
            $expr::App(_, _) | $expr::Subscript(_, _, _) => false,
            $expr::Op1(_, _, _) => matches!($child, $expr::Op2(_, _, _, _)),
            $expr::Op2(parent_op, _, _, _) => match $child {
                $expr::Const(_, _, _)
                | $expr::Var(_, _)
                | $expr::App(_, _)
                | $expr::Subscript(_, _, _)
                | $expr::If(_, _, _, _)
                | $expr::Op1(_, _, _) => false,
                // 3 * 2 + 1
                $expr::Op2(child_op, _, _, _) => {
                    // if we have `3 * (2 + 3)`, the parent's precedence
                    // is higher than the child and we need enclosing parens
                    parent_op.precedence() > child_op.precedence()
                }
            },
            $expr::If(_, _, _, _) => false,
        }
    }}
);

fn paren_if_necessary(parent: &Expr0, child: &Expr0, eqn: String) -> String {
    if child_needs_parens!(Expr0, parent, child, eqn) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

fn paren_if_necessary1(parent: &Expr1, child: &Expr1, eqn: String) -> String {
    if child_needs_parens!(Expr1, parent, child, eqn) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
}

struct PrintVisitor {}

impl Visitor<String> for PrintVisitor {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            IndexExpr0::StarRange(id, _) => format!("*:{}", id),
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        match expr {
            Expr0::Const(s, _, _) => s.clone(),
            Expr0::Var(id, _) => id.clone(),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr0::Op1(op, l, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "!",
                };
                format!("{}{}", op, l)
            }
            Expr0::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "mod",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{} {} {}", l, op, r)
            }
            Expr0::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("if ({}) then ({}) else ({})", cond, t, f)
            }
        }
    }
}

pub fn print_eqn(expr: &Expr0) -> String {
    let mut visitor = PrintVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_print_eqn() {
    assert_eq!(
        "a + b",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a + b * c",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Mul,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a * (b + c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Add,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Negative,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "!a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Not,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Positive,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        print_eqn(&Expr0::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "lookup(a, 1.0)",
        print_eqn(&Expr0::App(
            UntypedBuiltinFn(
                "lookup".to_string(),
                vec![
                    Expr0::Var("a".to_string(), Loc::new(7, 8)),
                    Expr0::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
                ]
            ),
            Loc::new(0, 14),
        ))
    );
}

struct LatexVisitor {}

impl LatexVisitor {
    fn walk_index(&mut self, expr: &IndexExpr1) -> String {
        match expr {
            IndexExpr1::Wildcard(_) => "*".to_string(),
            IndexExpr1::StarRange(id, _) => format!("*:{}", id),
            IndexExpr1::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr1::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr1) -> String {
        match expr {
            Expr1::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{{NaN}}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr1::Var(id, _) => {
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{}}}", id)
            }
            Expr1::App(builtin, _) => {
                let mut args: Vec<String> = vec![];
                walk_builtin_expr(builtin, |contents| {
                    let arg = match contents {
                        BuiltinContents::Ident(id, _loc) => format!("\\mathrm{{{}}}", id),
                        BuiltinContents::Expr(expr) => self.walk(expr),
                    };
                    args.push(arg);
                });
                let func = builtin.name();
                format!("\\operatorname{{{}}}({})", func, args.join(", "))
            }
            Expr1::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr1::Op1(op, l, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "\\neg ",
                };
                format!("{}{}", op, l)
            }
            Expr1::Op2(op, l, r, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                let r = paren_if_necessary1(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => {
                        return format!("{}^{{{}}}", l, r);
                    }
                    BinaryOp::Mul => "\\cdot",
                    BinaryOp::Div => {
                        return format!("\\frac{{{}}}{{{}}}", l, r);
                    }
                    BinaryOp::Mod => "%",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{} {} {}", l, op, r)
            }
            Expr1::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);

                format!(
                    "\\begin{{cases}}
                     {} & \\text{{if }} {} \\\\
                     {} & \\text{{else}}
                 \\end{{cases}}",
                    t, cond, f
                )
            }
        }
    }
}

pub fn latex_eqn(expr: &Expr1) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_latex_eqn() {
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Mul,
            Box::new(Expr1::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "(\\mathrm{a\\_c} - 1) \\cdot \\mathrm{b}",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Mul,
            Box::new(Expr1::Op2(
                BinaryOp::Sub,
                Box::new(Expr1::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr1::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr1::Op2(
            BinaryOp::Mul,
            Box::new(Expr1::Var("b".to_string(), Loc::new(5, 6))),
            Box::new(Expr1::Op2(
                BinaryOp::Sub,
                Box::new(Expr1::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr1::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-\\mathrm{a}",
        latex_eqn(&Expr1::Op1(
            UnaryOp::Negative,
            Box::new(Expr1::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr1::Op1(
            UnaryOp::Not,
            Box::new(Expr1::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr1::Op1(
            UnaryOp::Positive,
            Box::new(Expr1::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        latex_eqn(&Expr1::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "\\operatorname{lookup}(\\mathrm{a}, 1.0)",
        latex_eqn(&Expr1::App(
            BuiltinFn::Lookup(
                "a".to_string(),
                Box::new(Expr1::Const("1.0".to_owned(), 1.0, Default::default())),
                Default::default(),
            ),
            Loc::new(0, 14),
        ))
    );
}

#[cfg(test)]
mod dimension_vec_tests {
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
}
