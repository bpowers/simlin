// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::BTreeSet;

use crate::ast::{ArrayView, BinaryOp, Loc};
use crate::common::{Canonical, Ident, Result};
use crate::sim_err;

use super::dimensions::UnaryOp;

/// A reference to one slot of a model variable, **by name**.
///
/// This is the compiler's only variable-address representation. Lowering
/// produces it, codegen emits it into `symbolic::SymbolicOpcode` /
/// `SymbolicStaticView` / `SymbolicModuleDecl` operands unchanged, and
/// `symbolic::resolve_module` turns it into a concrete slot exactly once, at
/// assembly, against the model's final `VariableLayout`. Nothing in between
/// needs to know where the variable lives, which is what lets one salsa cache
/// entry per variable serve both the diagnostic pass and assembly and survive
/// unrelated variables being added, removed, or renamed.
///
/// `name` is the canonical name of the variable that owns the slot **in the
/// model being compiled**. A cross-module reference (`m·x`) resolves to the
/// *module* variable `m` with `element_offset` indexing into that module
/// instance's slot block, because the enclosing model's layout has one entry
/// for `m` spanning the whole sub-model and none for `m·x`. That is the one
/// place a sub-model's own (already fixed) layout is consulted during
/// lowering; see `Context::submodel_offset_within`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct VarRef {
    /// Canonical name of the variable that owns the referenced slot.
    pub name: Ident<Canonical>,
    /// Offset of the slot within that variable's storage: 0 for a scalar,
    /// `0..size` for an array element or a slot inside a module instance.
    pub element_offset: usize,
}

// A `VarRef` rides on `PerVarBytecodes`, whose `Debug` is unconditional (the
// fragment characterization goldens and the salsa artifacts print it), so this
// cannot hang off the optional `debug-derive` feature the way `Expr` does.
#[cfg(not(feature = "debug-derive"))]
impl std::fmt::Debug for VarRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

/// `name@element` -- the spelling the fragment characterization goldens and the
/// compiler's diagnostics both use for a reference.
impl std::fmt::Display for VarRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.element_offset)
    }
}

impl VarRef {
    /// A reference to the variable's first slot.
    pub(crate) fn base(name: Ident<Canonical>) -> Self {
        VarRef {
            name,
            element_offset: 0,
        }
    }

    pub(crate) fn new(name: Ident<Canonical>, element_offset: usize) -> Self {
        VarRef {
            name,
            element_offset,
        }
    }

    /// The reference `delta` slots further into the same variable.
    pub(crate) fn offset_by(&self, delta: usize) -> Self {
        VarRef {
            name: self.name.clone(),
            element_offset: self.element_offset + delta,
        }
    }

    /// Whether this reference is to the variable's *whole* storage, i.e. its
    /// first slot.
    ///
    /// Several codegen/lowering lookups are only meaningful for a whole-variable
    /// reference (the source extent of a VECTOR ELM MAP, the dimensions of a
    /// transposed bare array, the identity of a lookup table). Those used to ask
    /// "does any variable's storage *begin* at this offset?"; asking whether the
    /// reference sits at its owner's base is the same question, and answers it
    /// without a reverse scan.
    pub(crate) fn is_whole_var(&self) -> bool {
        self.element_offset == 0
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct Table {
    pub data: Vec<(f64, f64)>,
}

impl Table {
    pub(crate) fn new(ident: &str, t: &crate::variable::Table) -> Result<Self> {
        if t.x.len() != t.y.len() {
            return sim_err!(BadTable, ident.to_string());
        }

        let data: Vec<(f64, f64)> = t.x.iter().copied().zip(t.y.iter().copied()).collect();

        Ok(Self { data })
    }
}

pub(crate) type BuiltinFn = crate::builtins::BuiltinFn<Expr>;

/// Represents a single subscript index in a dynamic Subscript expression.
/// This enum distinguishes between single-element access and range access,
/// enabling proper bytecode generation for dynamic ranges.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
pub enum SubscriptIndex {
    /// Single element access - evaluates to a 1-based index
    Single(Expr),
    /// Range access - start and end expressions (1-based, inclusive)
    /// Used for dynamic ranges like arr[start:end] where bounds are variables
    Range(Expr, Expr),
}

impl SubscriptIndex {
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn strip_loc(self) -> Self {
        match self {
            SubscriptIndex::Single(expr) => SubscriptIndex::Single(expr.strip_loc()),
            SubscriptIndex::Range(start, end) => {
                SubscriptIndex::Range(start.strip_loc(), end.strip_loc())
            }
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Clone)]
#[allow(dead_code)]
pub enum Expr {
    Const(f64, Loc),
    Var(VarRef, Loc),
    /// Dynamic subscript with possible range indices
    /// (base reference, subscript indices, dimension sizes, location)
    Subscript(VarRef, Vec<SubscriptIndex>, Vec<usize>, Loc),
    StaticSubscript(VarRef, ArrayView, Loc), // base reference, precomputed view, location
    TempArray(u32, ArrayView, Loc),          // temp id, view into temp array, location
    TempArrayElement(u32, ArrayView, usize, Loc), // temp id, view, element index, location
    Dt(Loc),
    App(BuiltinFn, Loc),
    /// EvalModule(module_ident, model_name, input_set, args)
    /// input_set is needed to look up the correct compiled module when a model has multiple instantiations
    EvalModule(
        Ident<Canonical>,
        Ident<Canonical>,
        BTreeSet<Ident<Canonical>>,
        Vec<Expr>,
    ),
    ModuleInput(usize, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, Loc),
    Op1(UnaryOp, Box<Expr>, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, Loc),
    AssignCurr(VarRef, Box<Expr>),
    AssignNext(VarRef, Box<Expr>),
    AssignTemp(u32, Box<Expr>, ArrayView), // temp id, expression to evaluate, view info
}

impl Expr {
    pub(super) fn get_loc(&self) -> Loc {
        match self {
            Expr::Const(_, loc) => *loc,
            Expr::Var(_, loc) => *loc,
            Expr::Subscript(_, _, _, loc) => *loc,
            Expr::StaticSubscript(_, _, loc) => *loc,
            Expr::TempArray(_, _, loc) => *loc,
            Expr::TempArrayElement(_, _, _, loc) => *loc,
            Expr::Dt(loc) => *loc,
            Expr::App(_, loc) => *loc,
            Expr::EvalModule(_, _, _, _) => Loc::default(),
            Expr::ModuleInput(_, loc) => *loc,
            Expr::Op2(_, _, _, loc) => *loc,
            Expr::Op1(_, _, loc) => *loc,
            Expr::If(_, _, _, loc) => *loc,
            Expr::AssignCurr(_, _) => Loc::default(),
            Expr::AssignNext(_, _) => Loc::default(),
            Expr::AssignTemp(_, _, _) => Loc::default(),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr::Const(c, _loc) => Expr::Const(c, loc),
            Expr::Var(v, _loc) => Expr::Var(v, loc),
            Expr::Subscript(off, subscripts, bounds, _) => {
                let subscripts = subscripts
                    .into_iter()
                    .map(|expr| expr.strip_loc())
                    .collect();
                Expr::Subscript(off, subscripts, bounds, loc)
            }
            Expr::StaticSubscript(off, view, _) => Expr::StaticSubscript(off, view, loc),
            Expr::TempArray(id, view, _) => Expr::TempArray(id, view, loc),
            Expr::TempArrayElement(id, view, idx, _) => Expr::TempArrayElement(id, view, idx, loc),
            Expr::Dt(_) => Expr::Dt(loc),
            Expr::App(builtin, _loc) => {
                let builtin = match builtin {
                    // nothing to strip from these simple ones
                    BuiltinFn::Inf
                    | BuiltinFn::Pi
                    | BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => builtin,
                    BuiltinFn::IsModuleInput(id, _loc) => BuiltinFn::IsModuleInput(id, loc),
                    BuiltinFn::Lookup(id, a, _loc) => {
                        BuiltinFn::Lookup(id, Box::new(a.strip_loc()), loc)
                    }
                    BuiltinFn::LookupForward(id, a, _loc) => {
                        BuiltinFn::LookupForward(id, Box::new(a.strip_loc()), loc)
                    }
                    BuiltinFn::LookupBackward(id, a, _loc) => {
                        BuiltinFn::LookupBackward(id, Box::new(a.strip_loc()), loc)
                    }
                    BuiltinFn::Abs(a) => BuiltinFn::Abs(Box::new(a.strip_loc())),
                    BuiltinFn::Arccos(a) => BuiltinFn::Arccos(Box::new(a.strip_loc())),
                    BuiltinFn::Arcsin(a) => BuiltinFn::Arcsin(Box::new(a.strip_loc())),
                    BuiltinFn::Arctan(a) => BuiltinFn::Arctan(Box::new(a.strip_loc())),
                    BuiltinFn::Cos(a) => BuiltinFn::Cos(Box::new(a.strip_loc())),
                    BuiltinFn::Exp(a) => BuiltinFn::Exp(Box::new(a.strip_loc())),
                    BuiltinFn::Int(a) => BuiltinFn::Int(Box::new(a.strip_loc())),
                    BuiltinFn::Ln(a) => BuiltinFn::Ln(Box::new(a.strip_loc())),
                    BuiltinFn::Log10(a) => BuiltinFn::Log10(Box::new(a.strip_loc())),
                    BuiltinFn::Mean(args) => {
                        BuiltinFn::Mean(args.into_iter().map(|arg| arg.strip_loc()).collect())
                    }
                    BuiltinFn::Sign(a) => BuiltinFn::Sign(Box::new(a.strip_loc())),
                    BuiltinFn::Sin(a) => BuiltinFn::Sin(Box::new(a.strip_loc())),
                    BuiltinFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(a.strip_loc())),
                    BuiltinFn::Tan(a) => BuiltinFn::Tan(Box::new(a.strip_loc())),
                    BuiltinFn::Max(a, b) => {
                        BuiltinFn::Max(Box::new(a.strip_loc()), b.map(|b| Box::new(b.strip_loc())))
                    }
                    BuiltinFn::Min(a, b) => {
                        BuiltinFn::Min(Box::new(a.strip_loc()), b.map(|b| Box::new(b.strip_loc())))
                    }
                    BuiltinFn::Step(a, b) => {
                        BuiltinFn::Step(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::Pulse(a, b, c) => BuiltinFn::Pulse(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::Quantum(a, b) => {
                        BuiltinFn::Quantum(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::Ramp(a, b, c) => BuiltinFn::Ramp(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::SafeDiv(a, b, c) => BuiltinFn::SafeDiv(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::Sshape(a, b, c) => BuiltinFn::Sshape(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        Box::new(c.strip_loc()),
                    ),
                    BuiltinFn::Rank(a, direction) => {
                        BuiltinFn::Rank(Box::new(a.strip_loc()), Box::new(direction.strip_loc()))
                    }
                    BuiltinFn::Size(a) => BuiltinFn::Size(Box::new(a.strip_loc())),
                    BuiltinFn::Stddev(a) => BuiltinFn::Stddev(Box::new(a.strip_loc())),
                    BuiltinFn::Sum(a) => BuiltinFn::Sum(Box::new(a.strip_loc())),
                    BuiltinFn::VectorSelect(a, b, c, d, e) => BuiltinFn::VectorSelect(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        Box::new(c.strip_loc()),
                        Box::new(d.strip_loc()),
                        Box::new(e.strip_loc()),
                    ),
                    BuiltinFn::VectorElmMap(a, b) => {
                        BuiltinFn::VectorElmMap(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::VectorSortOrder(a, b) => {
                        BuiltinFn::VectorSortOrder(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::AllocateAvailable(a, b, c) => BuiltinFn::AllocateAvailable(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        Box::new(c.strip_loc()),
                    ),
                    BuiltinFn::AllocateByPriority(a, b, c, d, e) => BuiltinFn::AllocateByPriority(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        Box::new(c.strip_loc()),
                        Box::new(d.strip_loc()),
                        Box::new(e.strip_loc()),
                    ),
                    BuiltinFn::Previous(a, b) => {
                        BuiltinFn::Previous(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::Init(a) => BuiltinFn::Init(Box::new(a.strip_loc())),
                };
                Expr::App(builtin, loc)
            }
            Expr::EvalModule(id1, id2, input_set, args) => {
                let args = args.into_iter().map(|expr| expr.strip_loc()).collect();
                Expr::EvalModule(id1, id2, input_set, args)
            }
            Expr::ModuleInput(mi, _loc) => Expr::ModuleInput(mi, loc),
            Expr::Op2(op, l, r, _loc) => {
                Expr::Op2(op, Box::new(l.strip_loc()), Box::new(r.strip_loc()), loc)
            }
            Expr::Op1(op, r, _loc) => Expr::Op1(op, Box::new(r.strip_loc()), loc),
            Expr::If(cond, t, f, _loc) => Expr::If(
                Box::new(cond.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                loc,
            ),
            Expr::AssignCurr(off, rhs) => Expr::AssignCurr(off, Box::new(rhs.strip_loc())),
            Expr::AssignNext(off, rhs) => Expr::AssignNext(off, Box::new(rhs.strip_loc())),
            Expr::AssignTemp(id, rhs, view) => {
                Expr::AssignTemp(id, Box::new(rhs.strip_loc()), view)
            }
        }
    }
}

#[allow(dead_code)]
pub(super) fn decompose_array_temps(
    expr: Expr,
    next_temp_id: usize,
) -> Result<(Expr, Vec<Expr>, usize)> {
    Ok((expr, vec![], next_temp_id))
}
