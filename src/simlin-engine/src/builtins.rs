// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fmt;

/// Loc describes a location in an equation by the starting point and ending point.
/// Equations are strings typed by humans for a single variable -- u16 is long enough.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Default, Hash)]
pub struct Loc {
    pub start: u16,
    pub end: u16,
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.start, self.end)
    }
}

impl Loc {
    pub fn new(start: usize, end: usize) -> Self {
        Loc {
            start: start as u16,
            end: end as u16,
        }
    }

    /// union takes a second Loc and returns the inclusive range from the
    /// start of the earlier token to the end of the later token.
    pub fn union(&self, rhs: &Self) -> Self {
        Loc {
            start: self.start.min(rhs.start),
            end: self.end.max(rhs.end),
        }
    }
}

#[test]
fn test_loc_basics() {
    let a = Loc { start: 3, end: 7 };
    assert_eq!(a, Loc::new(3, 7));

    let b = Loc { start: 4, end: 11 };
    assert_eq!(Loc::new(3, 11), a.union(&b));

    let c = Loc { start: 1, end: 5 };
    assert_eq!(Loc::new(1, 7), a.union(&c));
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct UntypedBuiltinFn<Expr>(pub String, pub Vec<Expr>);

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum BuiltinFn<Expr> {
    Lookup(String, Box<Expr>, Loc),
    Abs(Box<Expr>),
    Arccos(Box<Expr>),
    Arcsin(Box<Expr>),
    Arctan(Box<Expr>),
    Cos(Box<Expr>),
    Exp(Box<Expr>),
    Inf,
    Int(Box<Expr>),
    IsModuleInput(String, Loc),
    Ln(Box<Expr>),
    Log10(Box<Expr>),
    Max(Box<Expr>, Box<Expr>),
    Mean(Vec<Expr>),
    Min(Box<Expr>, Box<Expr>),
    Pi,
    Pulse(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Ramp(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    SafeDiv(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Sin(Box<Expr>),
    Sqrt(Box<Expr>),
    Step(Box<Expr>, Box<Expr>),
    Tan(Box<Expr>),
    Time,
    TimeStep,
    StartTime,
    FinalTime,
}

impl<Expr> BuiltinFn<Expr> {
    pub fn name(&self) -> &'static str {
        match self {
            BuiltinFn::Lookup(_, _, _) => "lookup",
            BuiltinFn::Abs(_) => "abs",
            BuiltinFn::Arccos(_) => "arccos",
            BuiltinFn::Arcsin(_) => "arcsin",
            BuiltinFn::Arctan(_) => "arctan",
            BuiltinFn::Cos(_) => "cos",
            BuiltinFn::Exp(_) => "exp",
            BuiltinFn::Inf => "inf",
            BuiltinFn::Int(_) => "int",
            BuiltinFn::IsModuleInput(_, _) => "ismoduleinput",
            BuiltinFn::Ln(_) => "ln",
            BuiltinFn::Log10(_) => "log10",
            BuiltinFn::Max(_, _) => "max",
            BuiltinFn::Mean(_) => "mean",
            BuiltinFn::Min(_, _) => "min",
            BuiltinFn::Pi => "pi",
            BuiltinFn::Pulse(_, _, _) => "pulse",
            BuiltinFn::Ramp(_, _, _) => "ramp",
            BuiltinFn::SafeDiv(_, _, _) => "safediv",
            BuiltinFn::Sin(_) => "sin",
            BuiltinFn::Sqrt(_) => "sqrt",
            BuiltinFn::Step(_, _) => "step",
            BuiltinFn::Tan(_) => "tan",
            BuiltinFn::Time => "time",
            BuiltinFn::TimeStep => "time_step",
            BuiltinFn::StartTime => "initial_time",
            BuiltinFn::FinalTime => "final_time",
        }
    }
}

pub fn is_0_arity_builtin_fn(name: &str) -> bool {
    matches!(
        name,
        "inf" | "pi" | "time" | "time_step" | "dt" | "initial_time" | "final_time"
    )
}

pub fn is_builtin_fn(name: &str) -> bool {
    is_0_arity_builtin_fn(name)
        || matches!(
            name,
            "lookup"
                | "abs"
                | "arccos"
                | "arcsin"
                | "arctan"
                | "cos"
                | "exp"
                | "int"
                | "ismoduleinput"
                | "ln"
                | "log10"
                | "max"
                | "mean"
                | "min"
                | "pulse"
                | "ramp"
                | "safediv"
                | "sin"
                | "sqrt"
                | "step"
                | "tan"
        )
}

pub(crate) enum BuiltinContents<'a, Expr> {
    Ident(&'a str, Loc),
    Expr(&'a Expr),
}

pub(crate) fn walk_builtin_expr<'a, Expr, F>(builtin: &'a BuiltinFn<Expr>, mut cb: F)
where
    F: FnMut(BuiltinContents<'a, Expr>),
{
    match builtin {
        BuiltinFn::Inf
        | BuiltinFn::Pi
        | BuiltinFn::Time
        | BuiltinFn::TimeStep
        | BuiltinFn::StartTime
        | BuiltinFn::FinalTime => {}
        BuiltinFn::IsModuleInput(id, loc) => cb(BuiltinContents::Ident(id, *loc)),
        BuiltinFn::Lookup(id, a, loc) => {
            cb(BuiltinContents::Ident(id, *loc));
            cb(BuiltinContents::Expr(a));
        }
        BuiltinFn::Abs(a)
        | BuiltinFn::Arccos(a)
        | BuiltinFn::Arcsin(a)
        | BuiltinFn::Arctan(a)
        | BuiltinFn::Cos(a)
        | BuiltinFn::Exp(a)
        | BuiltinFn::Int(a)
        | BuiltinFn::Ln(a)
        | BuiltinFn::Log10(a)
        | BuiltinFn::Sin(a)
        | BuiltinFn::Sqrt(a)
        | BuiltinFn::Tan(a) => cb(BuiltinContents::Expr(a)),
        BuiltinFn::Mean(args) => {
            args.iter().for_each(|a| cb(BuiltinContents::Expr(a)));
        }
        BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) | BuiltinFn::Step(a, b) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
        }
        BuiltinFn::Pulse(a, b, c) | BuiltinFn::Ramp(a, b, c) | BuiltinFn::SafeDiv(a, b, c) => {
            cb(BuiltinContents::Expr(a));
            cb(BuiltinContents::Expr(b));
            match c {
                Some(c) => cb(BuiltinContents::Expr(c)),
                None => {}
            }
        }
    }
}

#[test]
fn test_is_builtin_fn() {
    assert!(is_builtin_fn("lookup"));
    assert!(!is_builtin_fn("lookupz"));
    assert!(is_builtin_fn("log10"));
}

#[test]
fn test_is_0_arity_builtin_fn() {
    assert!(!is_0_arity_builtin_fn("lookup"));
    assert!(is_0_arity_builtin_fn("time"));
}

#[test]
fn test_name() {
    enum TestExpr {}
    type Builtin = BuiltinFn<TestExpr>;

    assert_eq!("inf", Builtin::Inf.name());
    assert_eq!("time", Builtin::Time.name());
}
